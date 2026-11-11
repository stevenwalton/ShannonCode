//! **A stage → its ACTS**, read by the models over what the shell front end
//! gives (stages, the cwd each may run in, bindings, redirects, which words
//! still expand).
//!
//! A command is a set of acts, each on a target (design §8.1): one PROGRAM
//! act (the program, its subcommand chain, the flag nodes it touched and
//! their values); one PATH act per file role, at the operation the role
//! gives it (`cat F` reads F, `cp A B` reads A and writes B, `rm D` deletes
//! D, `./F` executes F); one HOST act per URL, at a direction (fetch, or
//! send when a body goes with it); one program act for each program a
//! flag RUNS (`find -exec rm` is also an `rm`, `--pager less` a `less`);
//! a CODE act for code the model does not read (`sh -c S`, `awk S`
//! beyond what the awk reader sees), or for a program named by
//! configuration (`--web` opens the browser, `git commit` the editor).
//! The decision (`permission::decide`, P4) judges each act by its rules;
//! the stage is its strictest act. Nothing here decides.
//!
//! **Placement.** A path act carries every absolute path it may name — one
//! per place the stage may run in (`Cwd::OneOf`, past a `cd` that may have
//! failed) — normalised. A word the gate cannot place (an expansion it
//! does not know, a glob with `..` past its wildcard, a cwd no `cd` could
//! be followed to) is an [`Act::Unplaced`], which asks. The one expansion
//! the gate does know is its own environment: the executor's table
//! (`exec::child_env`) is what the command will see in `$HOME`, `$TMPDIR`,
//! `$USER` and the rest, so `$TMPDIR/x` is placed under the scratch
//! directory — unless an earlier stage set a variable (`Stage::env_set`),
//! when nothing about the environment is certain (§8.7, the replay: seven
//! real prompts were `$HOME`/`$TMPDIR` words).
//!
//! **Unmodelled programs** have no roles and no arity: every word is a
//! possible flag, value, path and host at once. The program act keeps the
//! argv as a prefix (an allow is read as a literal prefix of it); every
//! non-dashed word is a path act at ANY operation (placed, so a denied
//! path is met); dashed words are unknown flags. The model is what lifts
//! the reading (G3).

use std::collections::BTreeMap;
use std::ops::Range;

use super::model::lang::{self, DYNAMIC};
use super::model::{
    Arity, At, DoubleDash, Effect, Flag, Model, ModelRef, Models, Node, Role, Source, Trust,
};
use crate::shell::pipeline::Bindings;
use crate::shell::{Cwd, RedirectKind, Stage};

/// A host act's direction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Fetch,
    Send,
}

/// A node a program act touched, for containment: an allow covers the act
/// only if every touch is covered — by the rule's own form, another allow,
/// a trailing `*`, or the model (a benign flag rides on a call that only
/// reads).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Touch {
    /// The canonical name (modelled) or the spelling (unmodelled).
    pub name: String,
    /// Every spelling the model gives it — what a rule may write.
    pub names: Vec<String>,
    pub value: Option<String>,
    /// Changes nothing past a read: on a call that only reads, it rides on
    /// a program allow without being named.
    pub benign: bool,
    /// No model knows it.
    pub unknown: bool,
    /// Why a guarded flag asks (the model's `why`).
    pub why: Option<String>,
    /// What the model says the flag does, when it says (`-delete` deletes).
    pub effect: Option<Effect>,
    /// The words it was read from — the flag and the values it took — by
    /// index in the argv read.
    pub at: Range<usize>,
    /// Those words in the line, as byte ranges: what the pane marks when a
    /// rule naming the flag decided the call (`deny rm --no-preserve-root`).
    /// Set where the argv's place in the line is known (`acts_of_argv`).
    pub written: Vec<Range<usize>>,
}

/// The program act: what ran, at which node of its tree, touching what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramAct {
    /// The basename.
    pub name: String,
    /// The subcommand chain below the program (`["pr", "view"]`).
    pub chain: Vec<String>,
    /// The effect of each node along the chain, the root first.
    pub chain_effects: Vec<Effect>,
    pub touches: Vec<Touch>,
    /// What a flag-value condition is checked against: the flags given,
    /// with their values, plus the facts they imply (`gh api -f` ⇒
    /// `method = POST`) and the endpoint, as `endpoint`.
    pub facts: Vec<(String, Option<String>)>,
    /// Operands with no role: an unmodelled program's words past the
    /// prefix, or a modelled position the model says nothing about.
    pub unknown_positionals: Vec<String>,
    pub modelled: bool,
    /// The node's effect, raised by what was touched.
    pub effect: Effect,
    /// The argv after the program, as the unmodelled prefix reading needs.
    pub prefix_words: Vec<String>,
    /// What only a rule naming this exact chain lifts ("prints a
    /// credential").
    pub needs_named: Vec<String>,
    /// This act is the inner argv of (program, chain): a rule on the outer
    /// program whose chain runs past the outer act's chain names it.
    pub via: Option<(String, Vec<String>)>,
    /// The act's last node takes an inner argv, so a longer rule chain may
    /// be naming the inner program.
    pub takes_argv: bool,
    /// A path-qualified program (`./x`, `/bin/ls`): the word, whose exec
    /// path act, if a rule allows it, covers the program act.
    pub exec_of: Option<String>,
    /// The model that read it — its trust and whose file it is — for the
    /// pane; `None` when unmodelled.
    pub model: Option<ModelRef>,
    /// The API call it makes, when its model names an endpoint (`gh api
    /// user`).
    pub call: Option<Call>,
    /// A wrapper the normalizer peeled (`timeout 5 CMD`, `nice CMD`): it
    /// runs the command inside, which is judged on its own acts, so with
    /// no rule naming it the wrapper itself is allowed.
    pub wrapper: bool,
    /// Read from an inner argv (`find -exec …`, `uv run …`), not the
    /// stage's own command: never the one a `re:` rule reads as the stage.
    pub inner: bool,
    /// Where it is written ([`Written`]): the program and subcommand words,
    /// and what makes the act what it is — a flag that raises its node
    /// (`-delete`, `-exec … ;`), a word no model knows, and the verb itself
    /// when its node is past a read (`rm`, `git push`) or no model reads it.
    pub written: Written,
}

/// Where an act is written in the line, as byte ranges, so the pane marks
/// the words it stands on rather than the whole command (Steven, 10-11:
/// "highlight less").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Written {
    /// The program and its subcommand words.
    pub verb: Vec<Range<usize>>,
    /// The words that make the act what it is: for a program
    /// ([`ProgramAct::written`]); else the word it was read from — a
    /// path's, a redirection's operator and target, a URL, code.
    pub cause: Vec<Range<usize>>,
}

/// A call to an API whose host the program reaches without naming it (`gh
/// api user` reaches `api.github.com`). Not an act — the implicit host is
/// the concession (design §10, 10-09) — but what the pane says instead:
/// the method, the host, the endpoint, and the model's gloss when it has
/// one ("the authenticated user's profile").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// Every method given or implied, in order: which one the program
    /// takes is its own, so the pane shows them all (`-X GET -X DELETE`).
    pub methods: Vec<String>,
    /// The host it names (`--hostname`), else the model's implicit one.
    pub host: Option<String>,
    pub endpoint: String,
    pub gloss: Option<String>,
}

/// A path act: a word, at an operation, at every place it may name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathAct {
    /// `None`: an unmodelled program's word — any operation.
    pub op: Option<Effect>,
    /// One per place the stage may run in, absolute, normalised.
    pub paths: Vec<String>,
    pub word: String,
    pub program: String,
    /// A redirection's target or a wrapper's argument, not a file the
    /// program itself names: a grant of it says nothing about what the
    /// program writes (review 2026-10-10, A: `git reset --hard >
    /// /work/scratch/log`), and a program rule's path words never cover it.
    pub redirect: bool,
    /// A listing (`Role::List`, in a call that stays a read): names and
    /// metadata reach the output, not what the files hold, so a guard
    /// BELOW the place is not met from above. Named outright, it is.
    pub listing: bool,
    /// The word it was read from (a redirection: its operator and target);
    /// none for a place no word names (`.`, read where it runs).
    pub at: Vec<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// Boxed: a program act carries its touches and facts, many times the
    /// size of the other variants, and a stage has a few acts at most.
    Program(Box<ProgramAct>),
    Path(PathAct),
    /// `at`, here and below: the word it was read from, as
    /// [`PathAct::at`].
    Host {
        dir: Dir,
        host: String,
        program: String,
        at: Vec<Range<usize>>,
    },
    /// Code the model does not read, or a program named by configuration.
    Code {
        program: String,
        what: String,
        at: Vec<Range<usize>>,
    },
    /// A privilege wrapper (`sudo`): a hard ask, lifted only by an allow
    /// naming the program it runs ([`Privileged`]).
    Privilege(Privileged),
    /// The stage cannot be judged soundly: disturbed bindings, an
    /// environment hazard, an argument list from a stream. Of the whole
    /// stage, so it is written nowhere in particular.
    Unsound(String),
    /// A word that could not be placed: an expansion the gate does not
    /// know, a cwd no `cd` could be followed to.
    Unplaced(String, Vec<Range<usize>>),
}

/// `act`, written at `at` where it has no place of its own yet: the word a
/// path, a host, code or an unplaced word was read from. A program act is
/// written by its own reading ([`ProgramAct::written`]); an unsound stage
/// is the whole stage.
fn written_at(mut act: Act, at: &[Range<usize>]) -> Act {
    let slot = match &mut act {
        Act::Path(p) => &mut p.at,
        Act::Host { at, .. } | Act::Code { at, .. } | Act::Unplaced(_, at) => at,
        Act::Program(_) | Act::Privilege(_) | Act::Unsound(_) => return act,
    };
    if slot.is_empty() {
        *slot = at.to_vec();
    }
    act
}

/// A privilege wrapper run, as its lift needs it (Steven, 10-11: "granting
/// sudo doesn't grant everything, only everything the user defines"): an
/// allow on the wrapper naming the program it runs and every flag given the
/// wrapper (`allow sudo -u=pg psql *`) lifts the ask; `allow sudo *` lifts
/// nothing; the path rules judge every file as ever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Privileged {
    /// The wrapper's basename, as written (`sudo`, or `Sudo` on a disk that
    /// folds case).
    pub wrapper: String,
    /// The basename of the program it runs, when it runs one.
    pub runs: Option<String>,
    /// The wrapper's own flags as Shannon's model of it reads them, each a
    /// touch with its value; `None` when nothing may lift it: a wrapper
    /// that takes its command as a shell string (`su -c`), a flag that hands
    /// the command to a shell (`sudo -s`, `-i`), or a flag Shannon does not
    /// know.
    pub own: Option<Vec<Touch>>,
    /// The wrapper's word in the line.
    pub at: Vec<Range<usize>>,
}

/// **Shannon's privilege wrappers**, each with the flags that hand its
/// command to a shell (`None`: it takes a shell string, and no rule lifts
/// it). Privilege is Shannon's fact, never a model's: a user's `sudo.toml`
/// reads sudo's words, and cannot say sudo raises nothing (review
/// 2026-10-11, E2). Kept equal to the shipped models' `privilege` marks by
/// `every_privilege_wrapper_is_marked_in_its_model`.
pub(crate) const PRIVILEGE_WRAPPERS: &[(&str, Option<&[&str]>)] = &[
    ("sudo", Some(&["-s", "--shell", "-i", "--login"])),
    ("doas", Some(&["-s"])),
    ("pkexec", Some(&[])),
    ("run0", Some(&[])),
    ("su", None),
];

/// Is `base` a privilege wrapper — Shannon's, or one a model marks (a
/// user's model may add the mark, never take Shannon's away)? Compared
/// without case: macOS runs `/usr/bin/sudo` for `Sudo`.
fn is_privileged(base: &str, cx: &Context) -> bool {
    PRIVILEGE_WRAPPERS
        .iter()
        .any(|(w, _)| w.eq_ignore_ascii_case(base))
        || cx.models.get(base).is_some_and(|m| m.privilege)
}

/// The privilege act of `wrapper`, written at `at`, given `own` (its words
/// before what it runs) and running `runs`.
fn privileged(wrapper: &str, own: &[String], runs: Option<&str>, at: Vec<Range<usize>>) -> Act {
    let shell_flags = PRIVILEGE_WRAPPERS
        .iter()
        .find(|(w, _)| w.eq_ignore_ascii_case(wrapper))
        .and_then(|(_, shell)| *shell);
    let own = shell_flags.and_then(|shell| {
        let name = wrapper.to_ascii_lowercase();
        let model = Models::shipped().get(&name)?;
        let words: Vec<String> = std::iter::once(name).chain(own.iter().cloned()).collect();
        let r = read(model, &words);
        let lifts = r.unknown_positionals.is_empty()
            && r.touches
                .iter()
                .all(|t| !t.unknown && !t.names.iter().any(|n| shell.contains(&n.as_str())));
        lifts.then_some(r.touches)
    });
    Act::Privilege(Privileged {
        wrapper: wrapper.to_string(),
        runs: runs.map(|r| r.rsplit('/').next().unwrap_or(r).to_string()),
        own,
        at,
    })
}

/// A program act, boxed into its variant.
fn program_act(act: ProgramAct) -> Act {
    Act::Program(Box::new(act))
}

/// What the reader needs besides the stage: where Shannon started, the
/// home a `~` means, the models, and the environment the command will run
/// in — the one expansion the gate knows.
pub struct Context<'a> {
    pub start: &'a str,
    pub home: &'a str,
    pub models: &'a Models,
    /// The executor's environment (`exec::child_env`), by name.
    pub env: &'a BTreeMap<String, String>,
}

// ---- paths -------------------------------------------------------------------

/// Lexically normalised: `a/./b` → `a/b`, `a/b/..` → `a`; a `..` that climbs
/// out of a relative path stays (`../x`), out of an absolute one is dropped.
pub fn normalize(path: &str) -> String {
    // One normaliser for the reader and the cwd tracker: the rule it
    // encodes (a `..` climbing out of an absolute path is dropped, out of
    // a relative one kept) is security history, and was written twice
    // (review 2026-10-10, C).
    crate::shell::pipeline::normalize_path(path)
}

/// `~` and `~/x` as `home`; anything else as written (`~user` is not ours
/// to place).
pub fn expand_home(path: &str, home: &str) -> String {
    if path == "~" {
        home.to_string()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else {
        path.to_string()
    }
}

/// Absolute and normalised, from `base` (absolute).
pub fn absolute(base: &str, path: &str, home: &str) -> String {
    let p = expand_home(path, home);
    if p.starts_with('/') {
        normalize(&p)
    } else {
        normalize(&format!("{base}/{p}"))
    }
}

fn wild(seg: &str) -> bool {
    seg.contains(['*', '?', '['])
}

/// The longest word the gate reads (a path is at most 4 KiB on every
/// system we run on), and the longest line; past either the stage is
/// unsound, not read.
const MAX_WORD_BYTES: usize = 4096;
const MAX_LINE_BYTES: usize = 65536;
/// How deep inner argvs nest before the gate stops reading (`find -exec
/// sh -c 'xargs …'`): past it the stage is unsound.
const MAX_INNER_DEPTH: usize = 8;

/// Where a path begins inside a short flag of an unmodelled program
/// (`-t/etc` → 2, `-C./sub` → 2): the first `/` or `./` past the letters.
/// `None` for a long flag, or a word with no path in it.
fn attached_path(w: &str) -> Option<usize> {
    if w.starts_with("--") || w.len() < 3 {
        return None;
    }
    let at = w.find('/')?;
    let at = if at > 0 && w[..at].ends_with('.') {
        at - 1
    } else {
        at
    };
    (at >= 2).then_some(at)
}

/// **The one expansion the gate knows: its own environment.** `$NAME`,
/// `${NAME}` and `${NAME:-default}` for a NAME in `env` become the value
/// the command will see. A NAME not in the table is NOT read as unset —
/// the shell sets names the table never holds (`_`, `OLDPWD`, a `for`
/// variable), so `${_:-README.md}` after `echo ~/.ssh/id_rsa` is the key
/// (review 2026-10-10, C) — and a name the shell owns is never read from
/// the table either, since the gate's `PWD` is not the command's. Any
/// other `$` stays, and the word stays unplaced. `None` when nothing was
/// substituted.
pub fn substitute_known(word: &str, env: &BTreeMap<String, String>) -> Option<String> {
    // Names the shell sets as it runs: their value in the gate's own
    // environment says nothing about the command's.
    fn shell_owned(name: &str) -> bool {
        matches!(
            name,
            "_" | "PWD"
                | "OLDPWD"
                | "SHLVL"
                | "RANDOM"
                | "LINENO"
                | "SECONDS"
                | "PPID"
                | "BASHPID"
                | "PIPESTATUS"
                | "REPLY"
                | "OPTIND"
                | "OPTARG"
                | "MACHTYPE"
                | "HOSTTYPE"
                | "OSTYPE"
                | "EPOCHSECONDS"
                | "EPOCHREALTIME"
                | "SRANDOM"
        ) || name.starts_with("BASH")
            || name.starts_with("ZSH")
            || name.starts_with("COMP_")
            || name.starts_with("FUNCNAME")
    }
    let known = |name: &str| (!shell_owned(name)).then(|| env.get(name)).flatten();
    fn name_end(s: &str) -> usize {
        let mut end = 0;
        for (i, c) in s.char_indices() {
            if (i == 0 && (c.is_ascii_alphabetic() || c == '_'))
                || (i > 0 && (c.is_ascii_alphanumeric() || c == '_'))
            {
                end = i + c.len_utf8();
            } else {
                break;
            }
        }
        end
    }
    let mut out = String::with_capacity(word.len());
    let mut rest = word;
    let mut changed = false;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        if let Some(inner) = after.strip_prefix('{') {
            let Some(close) = inner.find('}') else {
                out.push_str(&rest[at..]);
                return changed.then_some(out);
            };
            let body = &inner[..close];
            let n = name_end(body);
            let (name, tail) = body.split_at(n);
            let value = match (known(name), tail) {
                (Some(v), "") => Some(v.clone()),
                // `${NAME:-default}`: the value when the gate knows it and
                // it is not empty; the default when the gate knows it IS
                // empty. A name the gate does not know may be set by the
                // shell, so the word stays as written.
                (Some(v), d) if d.starts_with(":-") => {
                    if v.is_empty() {
                        Some(substitute_known(&d[2..], env).unwrap_or_else(|| d[2..].to_string()))
                    } else {
                        Some(v.clone())
                    }
                }
                _ => None,
            };
            match value {
                Some(v) if n > 0 => {
                    out.push_str(&v);
                    changed = true;
                    rest = &inner[close + 1..];
                }
                _ => {
                    out.push_str(&rest[at..=at + 1 + close]);
                    rest = &inner[close + 1..];
                }
            }
            continue;
        }
        let n = name_end(after);
        if n > 0
            && let Some(v) = known(&after[..n])
        {
            out.push_str(v);
            changed = true;
            rest = &after[n..];
        } else {
            out.push('$');
            rest = after;
        }
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// Where a word can be placed: itself, or a glob's literal prefix; `None`
/// for what may name anywhere (a live expansion, a glob climbing out).
fn placed(word: &str, live: bool) -> Option<String> {
    // A substitution is never a place, however the lexer handed it over:
    // `<(…)` is lowered to a literal `$()` (review 2026-10-10, E).
    if word.contains("$(") || word.contains('`') {
        return None;
    }
    if !live {
        return Some(word.to_string());
    }
    if word.contains(['$', '`', '{']) {
        return None;
    }
    let tilde_only =
        word == "~" || (word.starts_with("~/") && !word[1..].contains(['*', '?', '[', '$', '{']));
    if tilde_only {
        return Some(word.to_string());
    }
    // `~user/x`: another user's home, which the gate does not know.
    if word.starts_with('~') && !word.starts_with("~/") {
        return None;
    }
    if !word.contains(['*', '?', '[']) {
        return Some(word.to_string());
    }
    // A glob segment that may expand to `..` (`.*`, `.?`) can climb out of
    // the place its prefix names; `.env*` cannot, and is placed as itself.
    // A bracket class is the shell's, not `glob_match`'s (`.[.]` and
    // `.[!a]` are `..` to bash; `.[]e]nv` is `.env`), so a dot-led segment
    // holding one is placed nowhere (review 2026-10-10, A).
    let segs: Vec<&str> = word.split('/').collect();
    let first = segs.iter().position(|s| wild(s))?;
    if segs[first..].iter().any(|s| {
        *s == ".."
            || (s.starts_with('.')
                && wild(s)
                && (s.contains('[') || super::line::glob_match(s, "..", false)))
    }) {
        return None;
    }
    // Placed whole, glob and all: its literal prefix says where it is (inside
    // or outside), and the glob names the entries it picks, so a grant of
    // `/srv/out/*` covers `rm *.o` run there.
    Some(word.to_string())
}

/// The absolute places a stage may run in; `None` past a `cd` no reader
/// could follow.
fn places(stage: &Stage, cx: &Context) -> Option<Vec<String>> {
    let ps = stage.cwd.places()?;
    Some(
        ps.iter()
            .map(|p| match p {
                Cwd::Resolved(d) => absolute(cx.start, d, cx.home),
                _ => cx.start.to_string(),
            })
            .collect(),
    )
}

/// The path act for `word` at `op`, placed at every place the stage may
/// run in — or an [`Act::Unplaced`].
fn path_act(
    stage: &Stage,
    cx: &Context,
    word: &str,
    op: Option<Effect>,
    program: &str,
    live: bool,
) -> Option<Act> {
    if stage.home_set && word.starts_with('~') {
        return Some(Act::Unplaced(
            format!("{word} (HOME was set earlier on the line)"),
            Vec::new(),
        ));
    }
    // The gate's own environment is the one expansion it knows — unless an
    // earlier stage set a variable, when nothing about it is certain.
    let substituted = (live && !stage.env_set)
        .then(|| substitute_known(word, cx.env))
        .flatten();
    let (word_read, live) = match &substituted {
        Some(s) => (s.as_str(), s.contains(['$', '`', '*', '?', '[', '{'])),
        None => (word, live),
    };
    let Some(place) = placed(word_read, live) else {
        return Some(Act::Unplaced(word.to_string(), Vec::new()));
    };
    let Some(bases) = places(stage, cx) else {
        return Some(Act::Unplaced(
            format!("{word} (the directory it runs in is not known)"),
            Vec::new(),
        ));
    };
    let paths: Vec<String> = bases.iter().map(|b| absolute(b, &place, cx.home)).collect();
    // A device holds no one's files: `cat /dev/null`, `wc < /dev/fd/0` act
    // on nothing — to anything but a delete (`rm /dev/null` is not a write
    // to the void). Exact, on the normalised path: `/dev/fd/../../etc/x`
    // is `/etc/x`.
    if op != Some(Effect::Delete) && paths.iter().all(|p| is_device(p)) {
        return None;
    }
    Some(Act::Path(PathAct {
        op,
        paths,
        word: word.to_string(),
        program: program.to_string(),
        redirect: false,
        listing: false,
        at: Vec::new(),
    }))
}

/// `act`, marked a listing when `names_only` (a path act; any other act as
/// it is: an unplaced word is unplaced however it is read).
fn listed(act: Act, names_only: bool) -> Act {
    match act {
        Act::Path(p) => Act::Path(PathAct {
            listing: names_only,
            ..p
        }),
        other => other,
    }
}

/// A device, not a file: `/dev/null`, the standard streams, a descriptor.
fn is_device(path: &str) -> bool {
    matches!(
        path,
        "/dev/null" | "/dev/stdin" | "/dev/stdout" | "/dev/stderr" | "/dev/tty" | "/dev/zero"
    ) || path
        .strip_prefix("/dev/fd/")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

// ---- reading an argv with a model ---------------------------------------------

/// What one argv says, read by its model.
/// Indices, here, are of words in the argv read ([`Touch::at`]).
struct Reading {
    chain: Vec<String>,
    /// Where each subcommand word of the chain is.
    chain_at: Vec<usize>,
    chain_effects: Vec<Effect>,
    touches: Vec<Touch>,
    facts: Vec<(String, Option<String>)>,
    /// (role, word, the flag that gave it — none for a positional, where
    /// the word is) for every value and positional with a role; switches
    /// are applied over all of them once the argv is read (`tar -fc`: the
    /// mode may come after).
    roles: Vec<(Role, String, Option<String>, usize)>,
    /// A directory the program moves to before acting (`git -C`, `uv
    /// --directory`, `tar -C`): its relative words are placed under it.
    chdir: Option<String>,
    unknown_positionals: Vec<String>,
    /// Where each of `unknown_positionals` is.
    unknown_at: Vec<usize>,
    /// Inner argvs: from `-exec … ;`, an `argv` positional, an external
    /// word, a flag whose value names a program — each with where its
    /// words are (a value split into an argv: the value's word, for each).
    inners: Vec<(Vec<String>, Vec<usize>)>,
    effect: Effect,
    /// The effect of the node the argv ends at, before any flag or role
    /// raised it: what the program does by being this command (`rm`
    /// deletes; `find` reads, and `-delete` is what deletes).
    node_effect: Effect,
    needs_named: Vec<String>,
    /// Code the program runs that no reader sees, and where it is named.
    code: Vec<(String, Vec<usize>)>,
    writes: Vec<String>,
    takes_argv: bool,
    /// The endpoint named, and the model's gloss for it.
    endpoint: Option<(String, Option<String>)>,
    /// What the node the argv ends at reads where it runs (`Node::reads_here`).
    reads_here: Option<Role>,
}

/// The long option `spelling` abbreviates, under a family that takes a
/// prefix (`--no` → `--no-preserve-root`): the one spelling among the
/// nodes' flags that begins with it, when exactly one does and none is it.
fn abbreviated(nodes: &[&Node], spelling: &str, model: &Model) -> Option<String> {
    if !model.family.long_abbrev || !spelling.starts_with("--") || spelling.len() < 3 {
        return None;
    }
    if find_flag(nodes, spelling).is_some() {
        return None;
    }
    let mut found: Option<&str> = None;
    for node in nodes {
        for f in &node.flags {
            for name in f.names.iter().filter(|n| n.starts_with(spelling)) {
                match found {
                    Some(other) if other != name => return None,
                    _ => found = Some(name),
                }
            }
        }
    }
    found.map(str::to_string)
}

/// The flag `spelling` names, at the innermost node that has it.
fn find_flag<'a>(nodes: &[&'a Node], spelling: &str) -> Option<&'a Flag> {
    nodes.iter().rev().find_map(|n| {
        n.flags
            .iter()
            .find(|f| f.names.iter().any(|s| s == spelling))
    })
}

fn read(model: &Model, words: &[String]) -> Reading {
    let mut r = Reading {
        chain: Vec::new(),
        chain_at: Vec::new(),
        chain_effects: vec![model.root.effect],
        touches: Vec::new(),
        facts: Vec::new(),
        roles: Vec::new(),
        chdir: None,
        unknown_positionals: Vec::new(),
        unknown_at: Vec::new(),
        inners: Vec::new(),
        effect: Effect::Read,
        node_effect: Effect::Read,
        needs_named: Vec::new(),
        code: Vec::new(),
        writes: Vec::new(),
        takes_argv: false,
        endpoint: None,
        reads_here: None,
    };
    let mut nodes: Vec<&Node> = vec![&model.root];
    let mut positionals: Vec<(String, usize)> = Vec::new();
    let mut after_dd = false;
    let mut i = 1;
    // `at`: the words the flag was read from; `value_at`: the word its
    // value is in (the flag's own, when attached).
    let touch =
        |r: &mut Reading, spec: &Flag, value: Option<String>, at: Range<usize>, value_at: usize| {
            // An include glob (`rg -g`) overrides what the program skips —
            // benign only as an exclude (`-g '!tests'`).
            let includes = spec.role == Role::IncludeGlob
                && !value.as_deref().is_some_and(|v| v.starts_with('!'));
            let benign = spec.benign
                && spec.role.benign()
                && !includes
                && spec.effect.is_none_or(|e| e <= Effect::Read);
            r.touches.push(Touch {
                name: spec.canon.clone(),
                names: spec.names.clone(),
                value: value.clone(),
                benign,
                unknown: false,
                why: spec.why.clone(),
                effect: spec.effect,
                at: at.clone(),
                written: Vec::new(),
            });
            r.facts.push((spec.canon.clone(), value.clone()));
            if let Some(v) = &value
                && spec.role == Role::Program
            {
                // A program named by a flag's value is run: its own act.
                let argv: Vec<String> = v.split_whitespace().map(String::from).collect();
                let argv_at = vec![value_at; argv.len()];
                r.inners.push((argv, argv_at));
            } else if value.is_none()
                && spec.role == Role::Program
                && matches!(spec.arity, Arity::Zero)
            {
                // A flag that runs a program named by configuration (`--web`
                // opens $BROWSER, `-p` runs the pager): an act of its own,
                // which no blanket covers.
                r.code.push((
                    format!("`{}` runs a program named by configuration", spec.names[0]),
                    at.collect(),
                ));
            } else if let Some(v) = &value
                && !matches!(
                    spec.role,
                    Role::Data | Role::Enum | Role::Number | Role::Pattern
                )
            {
                r.roles
                    .push((spec.role, v.clone(), Some(spec.canon.clone()), value_at));
                if spec.chdir {
                    r.chdir = Some(v.clone());
                }
            } else if let Some(v) = &value
                && spec.role == Role::Data
                && let Some(file) = v.strip_prefix('@')
                && !file.is_empty()
            {
                // `@FILE` in a data value reads the file (curl's `-d @body`,
                // httpie's): a read act, so a credential named there is met.
                r.roles.push((
                    Role::ReadFile,
                    file.to_string(),
                    Some(spec.canon.clone()),
                    value_at,
                ));
            }
            if let Some(e) = spec.effect {
                r.effect = r.effect.max(e);
            }
        };
    while i < words.len() {
        let w = words[i].as_str();
        let node = *nodes.last().expect("the root is always there");
        if !after_dd && w == "--" && model.family.double_dash == DoubleDash::Terminator {
            after_dd = true;
            i += 1;
            continue;
        }
        // `find -- . -delete`: the `--` ends only its leading options, and
        // every primary after it is still read (review 28, E2).
        if w == "--" && model.family.double_dash == DoubleDash::Ignored {
            i += 1;
            continue;
        }
        if !after_dd && model.operators.iter().any(|o| o == w) {
            i += 1;
            continue;
        }
        let flag_like = !after_dd
            && w.starts_with('-')
            && w != "-"
            && (model.family.interspersed || positionals.is_empty());
        if flag_like {
            // Which spelling, and an inline value.
            let (spelling, inline) = match w.split_once('=') {
                Some((s, v)) if s.starts_with("--") || model.family.single_dash_long => {
                    (s, Some(v.to_string()))
                }
                _ => (w, None),
            };
            // GNU getopt takes any unambiguous prefix of a long option:
            // `rm --no /` is `--no-preserve-root` (SECURITY-2026-09-30 #4).
            let full = abbreviated(&nodes, spelling, model);
            let spelling = full.as_deref().unwrap_or(spelling);
            if (model.family.single_dash_long || spelling.starts_with("--"))
                && let Some(spec) = find_flag(&nodes, spelling)
            {
                i += consume(&mut r, spec, inline, words, i, &touch);
                continue;
            }
            if !spelling.starts_with("--") && model.family.bundling && w.len() > 1 {
                // A short cluster: letters until one takes a value.
                let letters: Vec<char> = w[1..].chars().collect();
                if let Some(numeric) = &model.family.numeric_option
                    && letters.iter().all(|c| c.is_ascii_digit())
                {
                    let value: String = letters.iter().collect();
                    r.touches.push(Touch {
                        name: numeric.clone(),
                        names: vec![w.to_string()],
                        value: Some(value.clone()),
                        benign: true,
                        unknown: false,
                        why: None,
                        effect: None,
                        at: i..i + 1,
                        written: Vec::new(),
                    });
                    r.facts.push((numeric.clone(), Some(value)));
                    i += 1;
                    continue;
                }
                // A letter the model does not know makes the cluster an
                // unknown flag (an allow must name the word), but the
                // letters it does know are still read out of it: a deny on
                // `-r` meets `rm -rfz`, and `tar -cPf/etc/x` still names the
                // file `-f` writes (SECURITY-2026-09-30 R2-5).
                let mut at = 0;
                let mut ok = true;
                let mut any_known = false;
                let mut advanced = 0;
                while at < letters.len() {
                    let one = format!("-{}", letters[at]);
                    let Some(spec) = find_flag(&nodes, &one) else {
                        ok = false;
                        at += 1;
                        continue;
                    };
                    any_known = true;
                    match spec.arity {
                        Arity::Zero => {
                            touch(&mut r, spec, None, i..i + 1, i);
                            at += 1;
                        }
                        _ => {
                            let rest: String = letters[at + 1..].iter().collect();
                            let inline = if rest.is_empty() { None } else { Some(rest) };
                            advanced = consume(&mut r, spec, inline, words, i, &touch) - 1;
                            at = letters.len();
                        }
                    }
                }
                if ok {
                    i += 1 + advanced;
                    continue;
                }
                // No letter known: the word as a whole is read below (an
                // unknown flag, or data to a program that prints its dashes).
                if any_known {
                    r.touches.push(Touch {
                        name: w.to_string(),
                        names: vec![w.to_string()],
                        value: None,
                        benign: false,
                        unknown: true,
                        why: model.unknown_flag_why.clone(),
                        effect: None,
                        at: i..i + 1 + advanced,
                        written: Vec::new(),
                    });
                    r.facts.push((w.to_string(), None));
                    // And as a flag with a value (`-z=1`), the way a rule
                    // about it is written: a deny on that meets it.
                    if let Some((n, v)) = w.split_once('=')
                        && n.len() > 1
                    {
                        r.facts.push((n.to_string(), Some(v.to_string())));
                    }
                    i += 1 + advanced;
                    continue;
                }
            }
            if !model.family.bundling
                && !model.family.single_dash_long
                && let Some(spec) = find_flag(&nodes, spelling)
            {
                i += consume(&mut r, spec, inline, words, i, &touch);
                continue;
            }
            // A short flag with its value attached, as getopt reads one
            // whether or not the program bundles (`awk -F:`, `awk
            // -f/x.awk`): the value is the flag's, its file placed — not an
            // unknown word (found by the model's own examples, held to the
            // reader at last).
            if !model.family.bundling
                && !model.family.single_dash_long
                && !spelling.starts_with("--")
                && let Some((k, _)) = w.char_indices().nth(2)
                && let Some(spec) = find_flag(&nodes, &w[..k])
                && matches!(spec.arity, Arity::One | Arity::OptionalAttached)
            {
                i += consume(&mut r, spec, Some(w[k..].to_string()), words, i, &touch);
                continue;
            }
            if model.family.dash_data {
                // echo: a dashed word it does not name is text it prints.
                positionals.push((w.to_string(), i));
                i += 1;
                continue;
            }
            r.touches.push(Touch {
                name: w.to_string(),
                names: vec![w.to_string()],
                value: None,
                benign: false,
                unknown: true,
                why: model.unknown_flag_why.clone(),
                effect: None,
                at: i..i + 1,
                written: Vec::new(),
            });
            // Its value, when written with `=`: a rule naming `--frob=1`
            // meets `--frob=1`, and the value is placed if it is a path.
            let (name, value) = match w.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(v.to_string())),
                None => (w.to_string(), None),
            };
            r.facts.push((name, value));
            i += 1;
            continue;
        }
        // A positional: a subcommand, an external program, an argv, or a word.
        if positionals.is_empty() && !after_dd {
            if let Some(sub) = node.sub(w) {
                r.chain.push(sub.name.clone());
                r.chain_at.push(i);
                r.chain_effects.push(sub.effect);
                nodes.push(sub);
                i += 1;
                continue;
            }
            if node.external && nodes.len() == 1 {
                r.inners
                    .push((words[i..].to_vec(), (i..words.len()).collect()));
                r.effect = r.effect.max(Effect::Exec);
                r.takes_argv = true;
                i = words.len();
                continue;
            }
        }
        let role_here = role_for(node, positionals.len(), &r.facts, None);
        if role_here == Some(Role::Argv) {
            r.inners
                .push((words[i..].to_vec(), (i..words.len()).collect()));
            r.effect = r.effect.max(Effect::Exec);
            r.takes_argv = true;
            i = words.len();
            continue;
        }
        positionals.push((w.to_string(), i));
        i += 1;
    }
    let node = *nodes.last().expect("the root is always there");
    r.reads_here = node.reads_here;
    let total = positionals.len();
    for (idx, (w, at)) in positionals.iter().enumerate() {
        match role_for(node, idx, &r.facts, Some(total)) {
            Some(role) => {
                r.roles.push((role, w.clone(), None, *at));
                r.effect = r.effect.max(role.effect());
                match role {
                    Role::Program => r.inners.push((vec![w.clone()], vec![*at])),
                    Role::Endpoint => {
                        r.facts.push(("endpoint".into(), Some(w.clone())));
                        let gloss = node.endpoints.get(w.trim_start_matches('/')).cloned();
                        r.endpoint = Some((w.clone(), gloss));
                    }
                    // Code is read by the language reader, which says what
                    // it does; unread code is an act of its own (`acts_of`).
                    _ => {}
                }
            }
            None => {
                r.unknown_positionals.push(w.clone());
                r.unknown_at.push(*at);
            }
        }
    }
    // The node's own effect, then its attributes.
    let given = |name: &str| r.facts.iter().any(|(n, _)| n == name);
    let mut node_effect = node.effect;
    if node.read_only_with.iter().any(|f| given(f)) {
        node_effect = Effect::Read;
    }
    r.node_effect = node_effect;
    r.effect = r.effect.max(node_effect);
    if let Some(unless) = &node.editor_unless
        && !unless.iter().any(|f| given(f))
    {
        // The command opens it (`git commit`): written where the command is.
        let verb = std::iter::once(0)
            .chain(r.chain_at.iter().copied())
            .collect();
        r.code
            .push(("the editor, a program named by configuration".into(), verb));
    }
    if node.prints_secret {
        r.needs_named.push("prints a credential".into());
    }
    r.writes.extend(node.writes.iter().cloned());
    // Implied facts: a flag's presence implies a fact when the fact's own
    // flag is absent (`gh api -f` ⇒ `method = POST`).
    let mut implied = Vec::new();
    for n in &nodes {
        for spec in &n.flags {
            if let Some((k, v)) = &spec.implies
                && given(&spec.canon)
                && !given(k)
                && !implied
                    .iter()
                    .any(|(name, _): &(String, Option<String>)| name == k)
            {
                implied.push((k.clone(), Some(v.clone())));
            }
        }
    }
    r.facts.extend(implied);
    r.takes_argv |= node.positionals.iter().any(|p| p.role == Role::Argv);
    r.takes_argv |= node.external && nodes.len() == 1;
    r.chain_effects.truncate(r.chain.len() + 1);
    r
}

/// Consume a flag's value(s) from `words` at `i`; how many words were used,
/// the flag word included. `touch` is told which words the flag was read
/// from, and which holds its value.
fn consume(
    r: &mut Reading,
    spec: &Flag,
    inline: Option<String>,
    words: &[String],
    i: usize,
    touch: &impl Fn(&mut Reading, &Flag, Option<String>, Range<usize>, usize),
) -> usize {
    // The words from the flag's to the `n`th after it, as far as there are.
    let upto = |n: usize| i..(i + 1 + n).min(words.len());
    match &spec.arity {
        Arity::Zero => {
            touch(r, spec, None, upto(0), i);
            1
        }
        Arity::One => match inline {
            Some(v) => {
                touch(r, spec, Some(v), upto(0), i);
                1
            }
            None => {
                let v = words.get(i + 1).cloned();
                touch(r, spec, v, upto(1), i + 1);
                2
            }
        },
        Arity::OptionalAttached => {
            touch(r, spec, inline, upto(0), i);
            1
        }
        Arity::Until(terms) => {
            let mut inner = Vec::new();
            let mut j = i + 1;
            while j < words.len() && !terms.iter().any(|t| *t == words[j]) {
                inner.push(words[j].clone());
                j += 1;
            }
            // The inner argv is its own act; the flag's value is not re-read.
            // Written from the flag through its terminator (`-exec … \;`).
            touch(r, spec, None, upto(j - i), i);
            if !inner.is_empty() {
                r.inners.push((inner, (i + 1..j).collect()));
            }
            (j + 1) - i
        }
        Arity::N(roles) => {
            let mut used = 1;
            for (k, role) in roles.iter().enumerate() {
                if let Some(v) = words.get(i + 1 + k) {
                    if k == 0 {
                        touch(r, spec, Some(v.clone()), upto(roles.len()), i + 1);
                    } else {
                        r.roles
                            .push((*role, v.clone(), Some(spec.canon.clone()), i + 1 + k));
                    }
                    used += 1;
                }
            }
            used
        }
    }
}

/// The role of the positional at `idx` of `total` under `node`: `last` for
/// the final one of several, an index unless a flag gave the word, else
/// the rest.
fn role_for(
    node: &Node,
    idx: usize,
    facts: &[(String, Option<String>)],
    total: Option<usize>,
) -> Option<Role> {
    let present = |names: &[String]| names.iter().any(|f| facts.iter().any(|(n, _)| n == f));
    let last = total.is_some_and(|t| idx + 1 == t && t > 1);
    if last && let Some(p) = node.positionals.iter().find(|p| matches!(p.at, At::Last)) {
        return Some(p.role);
    }
    if let Some(p) = node
        .positionals
        .iter()
        .find(|p| matches!(p.at, At::Index(i) if i == idx) && !present(&p.unless))
    {
        return Some(p.role);
    }
    node.positionals
        .iter()
        .find(|p| matches!(p.at, At::Rest))
        .map(|p| p.role)
}

/// A role rewritten by a flag's presence (`sed -i`: the files read become
/// files written; `tar -c`: the archive `-f` names is written), per the
/// model's switches — a switch `on` a flag rewrites that flag's values,
/// one with no `on` the positionals. Applied once the whole argv is read,
/// so `tar -fc` and `tar -cf` are one (review 2026-10-10, E1: applied to
/// positionals only, tar's switch flipped the members and not the archive).
fn switched(
    model: &Model,
    role: Role,
    from: Option<&str>,
    facts: &[(String, Option<String>)],
) -> Role {
    model
        .switches
        .iter()
        .find(|s| {
            s.from == role && s.on.as_deref() == from && facts.iter().any(|(n, _)| *n == s.when)
        })
        .map_or(role, |s| s.to)
}

/// Whether a `-c KEY=VAL` key names a program, per the model's
/// `kv_programs`: an exact key, a `prefix.`, or a `.suffix`.
fn names_a_program(model: &Model, key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    let listed = |list: &[String]| {
        list.iter().any(|p| {
            if let Some(prefix) = p.strip_suffix('.') {
                k.starts_with(prefix) && k.len() > prefix.len()
            } else if p.starts_with('.') {
                k.ends_with(p.as_str())
            } else {
                k == *p
            }
        })
    };
    // A list of the harmless, when the model gives one, decides: a key not
    // on it runs something (`kv_safe`).
    if !model.kv_safe.is_empty() {
        return !listed(&model.kv_safe);
    }
    listed(&model.kv_programs)
}

/// The host a URL names — with a scheme, or without one, as curl and
/// httpie take a bare `example.com/path`.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next().unwrap_or(host);
    // `[::1]:8080` is a bracketed IPv6 host; a trailing dot is the root
    // label and names the same host (`evil.example.` — review 2026-10-10,
    // B), lowered as a rule's host is.
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(v6),
        None => host.split(':').next().unwrap_or(host),
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

fn is_url(word: &str) -> bool {
    word.starts_with("http://") || word.starts_with("https://")
}

// ---- the acts of a stage ---------------------------------------------------------

/// Every act of a stage: the soundness of the stage first (bindings, the
/// environment, a stream-fed argv, a privilege wrapper), then the argv's
/// acts, then the redirections'.
pub fn acts_of(stage: &Stage, cx: &Context) -> Vec<Act> {
    let mut acts = Vec::new();
    let cmd = &stage.command;
    // Bounded before anything is read: a word longer than any path or a
    // line longer than any command is not something the gate reads, it is
    // something that would stop the gate (review 2026-10-10, B: a 256k
    // glob, 8,000 nested argvs). Every string it reads as a place: the
    // words, the environment's values, the redirections' targets. It
    // measured the words alone, and 32 KB as a redirection's target took
    // seconds (review stop 12, T8 H5). Heredoc data is scanned, not placed.
    let placed = || {
        (cmd.words.iter())
            .chain(cmd.env.values())
            .chain(stage.redirects.iter().map(|r| &r.target))
    };
    let longest = placed().map(String::len).max().unwrap_or(0);
    let total: usize = placed().map(String::len).sum();
    if longest > MAX_WORD_BYTES || total > MAX_LINE_BYTES {
        acts.push(Act::Unsound(format!(
            "a word of {longest} bytes in a line of {total}: longer than the gate reads"
        )));
        return acts;
    }
    if stage.bindings == Bindings::Disturbed {
        acts.push(Act::Unsound(
            "an earlier alias, eval, source or PATH change disturbed what the names run".into(),
        ));
    }
    for h in cmd.env_hazards() {
        acts.push(Act::Unsound(format!(
            "`{}` is set for it: {}",
            h.name, h.why
        )));
    }
    // Where each peeled wrapper is written: the next word as written that
    // is it, in order — assignments may stand between them (`env X=1 sudo
    // …`), and the peel keeps every word it takes as written.
    let wrapper_at: Vec<Vec<Range<usize>>> = {
        let mut from = 0;
        cmd.wrappers
            .iter()
            .map(|w| {
                let found = cmd.expansion[from.min(cmd.expansion.len())..]
                    .iter()
                    .position(|(x, _)| x == w)
                    .map(|p| from + p);
                found.map_or_else(Vec::new, |j| {
                    from = j + 1;
                    stage.word_spans.get(j).cloned().into_iter().collect()
                })
            })
            .collect()
    };
    let mut at = 0;
    for (k, w) in cmd.wrappers.iter().enumerate() {
        let base = w.rsplit('/').next().unwrap_or(w);
        let argc = cmd.wrapper_argc.get(k).copied().unwrap_or(0);
        let own = cmd.wrapper_args.get(at..at + argc).unwrap_or_default();
        at += argc;
        if is_privileged(base, cx) {
            let runs = cmd.wrappers.get(k + 1).unwrap_or(&cmd.program);
            acts.push(privileged(
                base,
                own,
                (!runs.is_empty()).then_some(runs),
                wrapper_at[k].clone(),
            ));
        }
    }
    if cmd.argv_from_stream().is_some() {
        acts.push(Act::Unsound(
            "its argument list comes from a stream (xargs): words the gate never sees".into(),
        ));
    }
    // A word the shell has yet to expand may become a FLAG (`$X` holding
    // `-delete`, `-{a,b}`), so the flags that run are not the ones read —
    // where a flag could matter: to a program that prints its dashed words
    // (`echo $PATH`) it is data, and to one no flag makes more dangerous
    // (`cat $F`) the word is only a file still to be placed.
    let model = cmd
        .program
        .rsplit('/')
        .next()
        .and_then(|base| cx.models.get(base));
    let dash_data = model.is_some_and(|m| m.family.dash_data);
    let flags_matter = model.is_none_or(Model::flags_matter);
    // The gate's own environment is the one expansion it knows: a word it
    // can substitute, and that then neither splits nor begins with a dash,
    // runs as read (`ls $HOME`, `cp x $TMPDIR/y`).
    let known = |w: &str| {
        !stage.env_set
            && substitute_known(w, cx.env)
                .is_some_and(|s| !crate::shell::splits_unquoted(&s) && !s.starts_with('-'))
    };
    if let Some(w) = cmd.hidden_flag()
        && !dash_data
        && flags_matter
        && !known(w)
    {
        acts.push(Act::Unsound(format!(
            "`{w}` may expand into a flag, so the flags that run are not the ones shown"
        )));
    }
    // A word whose expansion splits into several (an unquoted `$(…)` or
    // backtick): the words that run are not the ones read.
    if cmd.words_may_split
        && !dash_data
        && flags_matter
        && !cmd.words.iter().all(|w| !cmd.expands(w) || known(w))
    {
        acts.push(Act::Unsound(
            "a word's expansion may split into several, one of which may expand into a flag, \
             so the words that run are not the ones shown"
                .into(),
        ));
    }
    if cmd.words.is_empty() {
        // Only assignments: nothing runs, but the shell still opens every
        // redirection — `X=1 > f` truncates `f` (review 2026-10-10, E2).
        // (The values themselves are strings until a command reads them.)
        let live = |w: &str| cmd.expands(w);
        acts.extend(redirect_acts(stage, cx, "", &live));
        return acts;
    }
    // A wrapper the normalizer peeled (`timeout 5 CMD`, `sudo CMD`) is a
    // program act of its own — a rule may name it — that runs the command
    // inside; its own flags were read by the peel (an unknown one stops it,
    // and the wrapper is then the command). The command runs via the
    // innermost wrapper.
    let mut via = None;
    for (w, at) in cmd.wrappers.iter().zip(&wrapper_at) {
        let base = w.rsplit('/').next().unwrap_or(w).to_string();
        let model = cx.models.get(&base);
        // Named by a path, the wrapper is that file, run: an exec act where
        // it is (`./env ls` is not `env`).
        if w.contains('/') {
            acts.extend(
                path_act(stage, cx, w, Some(Effect::Exec), &base, cmd.expands(w))
                    .map(|a| written_at(a, at)),
            );
        }
        acts.push(program_act(ProgramAct {
            name: base.clone(),
            chain: Vec::new(),
            chain_effects: model.map(|m| vec![m.root.effect]).unwrap_or_default(),
            touches: Vec::new(),
            facts: Vec::new(),
            unknown_positionals: Vec::new(),
            modelled: model.is_some(),
            effect: Effect::Exec,
            prefix_words: Vec::new(),
            needs_named: Vec::new(),
            via: via.clone(),
            takes_argv: true,
            exec_of: w.contains('/').then(|| w.clone()),
            model: model.map(Model::reference),
            call: None,
            wrapper: true,
            inner: false,
            written: Written {
                verb: at.clone(),
                cause: Vec::new(),
            },
        }));
        via = Some((base, Vec::new()));
    }
    let live = |w: &str| cmd.expands(w);
    // Every field that can hold a path holds one the gate must place: a
    // wrapper's own argv (`flock ~/.ssh/id_rsa cat f` — it executes), and an
    // assignment's value that is a path (`GIT_SSH_KEY=~/.ssh/id_rsa git
    // push`). A credential named in either walked past the credential
    // policy once (Reviewer E).
    let base = cmd
        .program
        .rsplit('/')
        .next()
        .unwrap_or(&cmd.program)
        .to_string();
    for w in cmd.wrapper_args.iter().filter(|w| {
        // A number is a duration or a priority (`timeout 5`, `nice -n 5`),
        // never a file.
        (!w.starts_with('-') || w.as_str() == "-") && !w.chars().all(|c| c.is_ascii_digit())
    }) {
        // The wrapper's, not a file the program names: never a program
        // grant's to cover, nor a proposal's to fold in (review 2026-10-10,
        // E2: `flock /srv/lock rm build/x` taught `rm` a grant of the lock).
        acts.extend(
            path_act(stage, cx, w, None, &base, live(w)).map(|a| match a {
                Act::Path(mut p) => {
                    p.redirect = true;
                    Act::Path(p)
                }
                other => other,
            }),
        );
    }
    for v in cmd.env.values() {
        if v.contains('/') || v.starts_with('~') {
            acts.extend(path_act(stage, cx, v, None, &base, live(v)));
        }
    }
    let at: Vec<usize> = (0..cmd.words.len())
        .map(|i| stage.written_index(i))
        .collect();
    acts.extend(acts_of_argv(stage, cx, &cmd.words, &at, &live, via, 0));
    acts.extend(redirect_acts(stage, cx, &base, &live));
    acts
}

/// The stage's redirections as path acts at their operation, marked as the
/// shell's (`PathAct::redirect`), named for the program by its name as
/// every other act of the stage is (`/bin/echo x > f` is `echo`'s write —
/// review 2026-10-10, A). One reader for a stage with a command and one
/// with only assignments: `X=1 > f` truncates `f` too (E2).
fn redirect_acts(
    stage: &Stage,
    cx: &Context,
    program: &str,
    live: &dyn Fn(&str) -> bool,
) -> Vec<Act> {
    let mut acts = Vec::new();
    for r in &stage.redirects {
        if r.is_null() {
            continue;
        }
        let op = match r.kind {
            RedirectKind::Read => Effect::Read,
            RedirectKind::Write | RedirectKind::Append => Effect::Write,
        };
        let redirected = path_act(stage, cx, &r.target, Some(op), program, live(&r.target));
        acts.extend(
            redirected.map(|a| match written_at(a, std::slice::from_ref(&r.span)) {
                Act::Path(mut p) => {
                    p.redirect = true;
                    Act::Path(p)
                }
                other => other,
            }),
        );
    }
    acts
}

/// The byte ranges of the words at indices `ks` of an argv, through `at`
/// (each word's index among the stage's words as written): sorted, each
/// once.
fn spans_of(stage: &Stage, at: &[usize], ks: impl IntoIterator<Item = usize>) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = ks
        .into_iter()
        .filter_map(|k| at.get(k).and_then(|&j| stage.word_spans.get(j)).cloned())
        .collect();
    out.sort_by_key(|r| (r.start, r.end));
    out.dedup();
    out
}

/// The acts of one argv — the stage's, or an inner one a flag or a
/// positional runs (`via` names the outer program and chain). `at[k]` is
/// where `words[k]` is among the stage's words as written
/// ([`Stage::written_index`]), so each act says where it is written.
pub fn acts_of_argv(
    stage: &Stage,
    cx: &Context,
    words: &[String],
    at: &[usize],
    live: &dyn Fn(&str) -> bool,
    via: Option<(String, Vec<String>)>,
    depth: usize,
) -> Vec<Act> {
    let mut acts = Vec::new();
    let Some(program) = words.first().cloned() else {
        return acts;
    };
    let word_at = |k: usize| spans_of(stage, at, [k]);
    if is_url(&program) {
        if let Some(h) = host_of(&program) {
            acts.push(Act::Host {
                dir: Dir::Fetch,
                host: h,
                program: "download".into(),
                at: word_at(0),
            });
        }
        acts.push(Act::Code {
            program: program.clone(),
            what: "a downloaded script, run".into(),
            at: word_at(0),
        });
        return acts;
    }
    let base = program.rsplit('/').next().unwrap_or(&program).to_string();
    let exec_of = if program.contains('/') {
        acts.extend(
            path_act(
                stage,
                cx,
                &program,
                Some(Effect::Exec),
                &base,
                live(&program),
            )
            .map(|a| written_at(a, &word_at(0))),
        );
        Some(program.clone())
    } else {
        None
    };
    // A privilege wrapper is a hard ask wherever it runs — as the stage's
    // command, or as an inner argv (`find . -exec sudo ls \;`): the
    // `via.is_none()` that once stood here let an allow on the outer
    // program run the inner `sudo` unasked, and no test held it (review
    // 2026-10-10, A).
    if is_privileged(&base, cx) {
        // Its own words are those before the command it runs, as Shannon's
        // model of it reads them (unread, nothing lifts it).
        let theirs = PRIVILEGE_WRAPPERS
            .iter()
            .find(|(w, _)| w.eq_ignore_ascii_case(&base))
            .and_then(|(w, _)| Models::shipped().get(w));
        let inner = theirs.and_then(|m| read(m, words).inners.into_iter().next());
        let end = words
            .len()
            .saturating_sub(inner.as_ref().map_or(0, |(w, _)| w.len()))
            .max(1);
        let own = &words[1..end];
        acts.push(privileged(
            &base,
            own,
            inner
                .as_ref()
                .and_then(|(w, _)| w.first())
                .map(String::as_str),
            word_at(0),
        ));
    }
    let Some(model) = cx.models.get(&base) else {
        // Unmodelled: a prefix for allows; every non-dashed word a path at
        // any operation; dashed words unknown flags.
        let mut touches = Vec::new();
        let mut facts = Vec::new();
        let mut unknown_positionals = Vec::new();
        for (k, w) in words.iter().enumerate().skip(1) {
            if w.starts_with('-') && w != "-" {
                touches.push(Touch {
                    name: w.clone(),
                    names: vec![w.clone()],
                    value: None,
                    benign: false,
                    unknown: true,
                    why: None,
                    effect: None,
                    at: k..k + 1,
                    written: word_at(k),
                });
                facts.push(flag_and_value(w));
            } else {
                unknown_positionals.push(w.clone());
            }
            let here = word_at(k);
            acts.extend(
                unmodelled_places(stage, cx, w, &base, live)
                    .into_iter()
                    .map(|a| written_at(a, &here)),
            );
        }
        // Unmodelled, it may act on where it runs (`cd /tmp && make`): the
        // place is a path act at no operation, inside or outside.
        acts.extend(path_act(stage, cx, ".", None, &base, false));
        acts.push(program_act(ProgramAct {
            name: base,
            chain: Vec::new(),
            chain_effects: Vec::new(),
            touches,
            facts,
            unknown_positionals,
            modelled: false,
            effect: Effect::Exec,
            prefix_words: words[1..].to_vec(),
            needs_named: Vec::new(),
            via,
            takes_argv: false,
            exec_of,
            model: None,
            call: None,
            wrapper: false,
            inner: depth > 0,
            // No model reads it: the program is what is not known, not
            // each of its words.
            written: Written {
                verb: word_at(0),
                cause: word_at(0),
            },
        }));
        return acts;
    };
    // Where this program's own reading begins (`Trust::Completion` below).
    let own_acts = acts.len();
    // An inner `xargs` (`find -exec xargs rm`, `uv run xargs rm`) builds
    // its argv from the input the gate never sees, as a peeled one does
    // (review 2026-10-10, E1); the stage's own peel is read by `acts_of`.
    if model.argv_from_stdin && depth > 0 {
        acts.push(Act::Unsound(
            "its argument list comes from a stream (xargs): words the gate never sees".into(),
        ));
    }
    let mut r = read(model, words);
    for t in &mut r.touches {
        t.written = spans_of(stage, at, t.at.clone());
    }
    // Over every role, now the whole argv is read: the switches (`tar -fc`
    // and `tar -cf` are one), and the directory the program moved to —
    // its relative words are placed under it, as after a `cd` (review
    // 2026-10-10, E1: `git -C /srv rm -r build` was placed at the start).
    let facts = r.facts.clone();
    let chdir = r.chdir.clone();
    let mut raised = r.effect;
    for (role, w, from, _) in &mut r.roles {
        *role = switched(model, *role, from.as_deref(), &facts);
        raised = raised.max(role.effect());
        if let Some(d) = &chdir
            && !(matches!(role, Role::Dir | Role::List) && w == d)
            && !w.starts_with('/')
            && !w.starts_with('~')
        {
            *w = format!("{d}/{w}");
        }
    }
    r.effect = raised;
    // Hosts, and the send direction: a body, an upload, or a method that
    // writes — ANY method given or implied, not the first: a program may take
    // the last (`curl -X GET -X DELETE` deletes; review 2026-10-11, E1), and
    // which one it takes is the program's, not the gate's to pick.
    let sends = r.facts.iter().any(|(n, v)| match n.as_str() {
        "data" | "form" | "upload-file" | "json" => true,
        "method" | "request" => v.as_deref().is_some_and(|m| {
            !matches!(m.to_ascii_uppercase().as_str(), "GET" | "HEAD" | "OPTIONS")
        }),
        _ => false,
    });
    // And a call that sends is a write, whatever node it is at: `gh api`
    // is a read node whose host is no act (implicit hosts are glosses,
    // 10-09), so `gh api -X PATCH …` and `gh api -f title=x …` ran with no
    // rule, unattended (review 2026-10-11, E1).
    if sends {
        r.effect = r.effect.max(Effect::Write);
    }
    let dir = if sends { Dir::Send } else { Dir::Fetch };
    // **A listing reaches names only while the call stays a read.** Once a
    // flag deletes, writes or runs (`find -delete`, `find -exec`, `fd -x`),
    // its places are acted on below, and reach what is there as ever:
    // under `allow find *`, `find . -delete` would otherwise delete
    // Shannon's state with nothing met from above.
    let names_only = r.effect <= Effect::Read;
    // **A walk that deletes deletes what is below where it starts** (`find
    // . -delete`): its roots are deletes, as `rm -r`'s operands are, for the
    // path rules to judge. A program rule grants no place, yet `allow find
    // *` alone deleted the project unasked while `allow rm *` asked for `rm
    // -rf build` (10-13). Asked of the node and the flags given, not of the
    // effect, which a run (`-exec`) outranks.
    let walk_deletes = r.node_effect == Effect::Delete
        || r.touches.iter().any(|t| t.effect == Some(Effect::Delete));
    let mut dirs: Vec<String> = Vec::new();
    for (role, w, _, k) in &r.roles {
        // Every act of this role is written at its word.
        let mut here: Vec<Act> = Vec::new();
        match role {
            Role::ReadFile => {
                here.extend(path_act(stage, cx, w, Some(Effect::Read), &base, live(w)))
            }
            Role::Dir => {
                dirs.push(w.clone());
                // The directory it moves to (`git -C keys status`) is where
                // it runs: read as the node reads where it runs.
                let chdir = r.chdir.as_ref() == Some(w) && r.reads_here == Some(Role::List);
                let act = path_act(stage, cx, w, Some(Effect::Read), &base, live(w));
                here.extend(act.map(|a| listed(a, chdir && names_only)));
            }
            Role::List => {
                dirs.push(w.clone());
                let op = match walk_deletes {
                    true => Effect::Delete,
                    false => Effect::Read,
                };
                let act = path_act(stage, cx, w, Some(op), &base, live(w));
                here.extend(act.map(|a| listed(a, names_only)));
            }
            Role::WriteFile => {
                here.extend(path_act(stage, cx, w, Some(Effect::Write), &base, live(w)))
            }
            Role::DeleteFile => {
                here.extend(path_act(stage, cx, w, Some(Effect::Delete), &base, live(w)))
            }
            Role::ExecFile => {
                here.extend(path_act(stage, cx, w, Some(Effect::Exec), &base, live(w)))
            }
            Role::Url => match host_of(w) {
                Some(h) => here.push(Act::Host {
                    dir,
                    host: h,
                    program: base.clone(),
                    at: Vec::new(),
                }),
                None => here.push(Act::Unplaced(w.clone(), Vec::new())),
            },
            Role::Host => here.push(Act::Host {
                dir,
                host: w.clone(),
                program: base.clone(),
                at: Vec::new(),
            }),
            Role::Kv => {
                let (k, v) = w.split_once('=').unwrap_or((w, ""));
                if names_a_program(model, k) {
                    here.push(Act::Code {
                        program: base.clone(),
                        what: format!("`{k}` names a program it runs"),
                        at: Vec::new(),
                    });
                } else if let Some(file) = v.strip_prefix('@').or_else(|| v.strip_prefix('<')) {
                    // curl's `-F name=@file` attaches the file, `name=<file`
                    // its contents: a read either way.
                    here.extend(path_act(
                        stage,
                        cx,
                        file,
                        Some(Effect::Read),
                        &base,
                        live(file),
                    ));
                }
            }
            Role::Code => {
                let (code, writes) = code_acts(stage, cx, model, w, &base);
                if writes {
                    // The script writes, so the program is a writer: with no
                    // rule it asks, as `sed -i` does.
                    r.effect = r.effect.max(Effect::Write);
                }
                here.extend(code);
            }
            // A program is in `inners`; an endpoint is a fact; the rest are
            // words.
            _ => {}
        }
        let written = word_at(*k);
        acts.extend(here.into_iter().map(|a| written_at(a, &written)));
    }
    for (what, ks) in &r.code {
        acts.push(Act::Code {
            program: base.clone(),
            what: what.clone(),
            at: spans_of(stage, at, ks.iter().copied()),
        });
    }
    // A flag the model does not know, with a value (`cat --file=~/.ssh/
    // id_rsa`, `cat --frob=.env`): each value it may carry is placed, at no
    // operation, as an unmodelled program's is — so a credential named
    // through it is met, with or without a `/` (review 2026-10-11, E1).
    for t in r.touches.iter().filter(|t| t.unknown) {
        let written = spans_of(stage, at, t.at.clone());
        acts.extend(
            unmodelled_places(stage, cx, &t.name, &base, live)
                .into_iter()
                .filter(|a| !matches!(a, Act::Path(p) if p.word == t.name))
                .map(|a| written_at(a, &written)),
        );
    }
    for w in &r.writes {
        acts.extend(path_act(stage, cx, w, Some(Effect::Write), &base, false));
    }
    // A program that reads where it runs when no operand names a place —
    // listed (`ls`, `git status`) or searched (`rg`, `git diff`).
    if let Some(here) = r.reads_here
        && !r
            .roles
            .iter()
            .any(|(role, _, _, _)| matches!(role, Role::Dir | Role::List | Role::ReadFile))
    {
        // A walk of where it runs deletes there as one of a root does
        // (`find -delete`).
        let op = match here == Role::List && walk_deletes {
            true => Effect::Delete,
            false => Effect::Read,
        };
        let act = path_act(stage, cx, ".", Some(op), &base, false);
        acts.extend(act.map(|a| listed(a, here == Role::List && names_only)));
        dirs.push(".".into());
    }
    // A completion model says what the words ARE, not what they do (an
    // import of a shell-completion spec): the operations its roles give its
    // places back no grant — they are at no operation, as an unmodelled
    // program's, so only `:*` grants them and every fence meets them. (A
    // redirection is the shell's, and keeps its own: `acts_of` adds it after
    // this argv's acts, never among them.)
    if model.trust == Trust::Completion {
        for act in &mut acts[own_acts..] {
            if let Act::Path(p) = act {
                p.op = None;
                // Nor that it only lists: a word at no operation may be
                // anything, below as well.
                p.listing = false;
            }
        }
        // And its roles may call a file data: every word is placed as an
        // unmodelled program's is, beside what the model placed, so no role
        // hides a place from a fence (review 2026-10-11, E1: `bat
        // /srv/keep/a` under `deny :* /srv/keep`, `bat .env`).
        let placed: Vec<String> = acts[own_acts..]
            .iter()
            .filter_map(|a| match a {
                Act::Path(p) => Some(p.word.clone()),
                _ => None,
            })
            .collect();
        for (k, w) in words.iter().enumerate().skip(1) {
            if !placed.contains(w) {
                let written = word_at(k);
                acts.extend(
                    unmodelled_places(stage, cx, w, &base, live)
                        .into_iter()
                        .map(|a| written_at(a, &written)),
                );
            }
        }
    }
    // Where implementations differ on a flag's value (`or_attached`: BSD
    // sed's `-i SUFFIX`, GNU's `-i[SUFFIX]`), the argv read the other way
    // too: every place and every code either reading finds is the call's
    // (review 2026-10-11, E2: BSD's reading of `sed -i s/x/y/ f` took `f`
    // for the script, and GNU sed edits it). The other reading's program
    // act is this one's, read once.
    if let Some(alt) = attached_reading(model) {
        let mut other = cx.models.clone();
        other.insert(alt);
        let alt_cx = Context {
            start: cx.start,
            home: cx.home,
            models: &other,
            env: cx.env,
        };
        for a in acts_of_argv(stage, &alt_cx, words, at, live, via.clone(), depth) {
            if !matches!(a, Act::Program(_)) && !acts.contains(&a) {
                acts.push(a);
            }
        }
    }
    let chain = r.chain.clone();
    // (argv, where each of its words is in this argv, the chain it runs from)
    let mut inners: Vec<(Vec<String>, Vec<usize>, Vec<String>)> = r
        .inners
        .iter()
        .map(|(i, ks)| (i.clone(), ks.clone(), chain.clone()))
        .collect();
    // **Shannon's facts stand under a user's model**: what runs code, runs
    // a file or a program, or prints a credential is the union of its
    // reading and Shannon's — a model may add caution, never take it away
    // (Steven, 10-11; review 2026-10-11, E2: a user `eval.toml` of a read ran
    // `eval 'rm -rf src'` unasked). Added after the completion pass: these
    // are Shannon's, and keep their operations. A trusted project's model
    // too: it replaces Shannon's by name as yours does (Steven, 10-13).
    if matches!(model.source, Source::User(_) | Source::Project(_))
        && let Some(theirs) = Models::shipped().get(&base)
    {
        let s = read(theirs, words);
        if s.effect == Effect::Exec {
            r.effect = Effect::Exec;
        }
        for n in s.needs_named {
            if !r.needs_named.contains(&n) {
                r.needs_named.push(n);
            }
        }
        for (what, ks) in s.code {
            acts.push(Act::Code {
                program: base.clone(),
                what,
                at: spans_of(stage, at, ks),
            });
        }
        for (role, w, _, k) in &s.roles {
            let mut here: Vec<Act> = Vec::new();
            match role {
                Role::Code => {
                    let (code, writes) = code_acts(stage, cx, theirs, w, &base);
                    if writes {
                        r.effect = r.effect.max(Effect::Write);
                    }
                    here.extend(code);
                }
                Role::ExecFile => {
                    here.extend(path_act(stage, cx, w, Some(Effect::Exec), &base, live(w)))
                }
                Role::Kv => {
                    let key = w.split_once('=').map_or(w.as_str(), |(k, _)| k);
                    if names_a_program(theirs, key) {
                        here.push(Act::Code {
                            program: base.clone(),
                            what: format!("`{key}` names a program it runs"),
                            at: Vec::new(),
                        });
                    }
                }
                _ => {}
            }
            let written = word_at(*k);
            acts.extend(here.into_iter().map(|a| written_at(a, &written)));
        }
        for (inner, ks) in s.inners {
            if !inners.iter().any(|(i, _, _)| *i == inner) {
                inners.push((inner, ks, s.chain.clone()));
            }
        }
    }
    // **Where the program act is written.** The verb: the program and its
    // subcommand words. The cause: what makes the act what it is — a flag
    // the model says does more than its node (`find . -delete`: the
    // `-delete`, not the `find`; `-exec … \;` through its terminator), a
    // word no model knows; and the verb itself when the node is past a read
    // (`rm`, `git push`) or nothing named explains the act (`curl -X POST`
    // sends by a method, which the model gives no effect).
    let verb_at: Vec<usize> = std::iter::once(0)
        .chain(r.chain_at.iter().copied())
        .collect();
    let raising: Vec<usize> = r
        .touches
        .iter()
        .filter(|t| t.unknown || t.effect.is_some_and(|e| e > r.node_effect))
        .flat_map(|t| t.at.clone())
        .chain(r.unknown_at.iter().copied())
        .collect();
    let verb_is_cause =
        r.node_effect > Effect::Read || (raising.is_empty() && r.effect > Effect::Read);
    let written = Written {
        verb: spans_of(stage, at, verb_at.iter().copied()),
        cause: spans_of(
            stage,
            at,
            raising
                .iter()
                .copied()
                .chain(verb_at.iter().copied().filter(|_| verb_is_cause)),
        ),
    };
    // The call it makes, for the pane: every method given or implied — none
    // is a fetch, as `sends` reads it — at the host it names, else its own.
    let call = r.endpoint.map(|(endpoint, gloss)| {
        let mut methods: Vec<String> = r
            .facts
            .iter()
            .filter(|(n, _)| matches!(n.as_str(), "method" | "request"))
            .filter_map(|(_, v)| v.clone())
            .collect();
        methods.dedup();
        if methods.is_empty() {
            methods.push("GET".into());
        }
        let named = r.roles.iter().find(|(role, ..)| *role == Role::Host);
        Call {
            methods,
            host: named
                .map(|(_, w, ..)| w.clone())
                .or_else(|| model.hosts.first().cloned()),
            endpoint,
            gloss,
        }
    });
    acts.push(program_act(ProgramAct {
        name: base.clone(),
        chain: r.chain,
        chain_effects: r.chain_effects,
        touches: r.touches,
        facts: r.facts,
        unknown_positionals: r.unknown_positionals,
        modelled: true,
        effect: r.effect,
        prefix_words: words[1..].to_vec(),
        needs_named: r.needs_named,
        via,
        takes_argv: r.takes_argv,
        exec_of,
        model: Some(model.reference()),
        call,
        wrapper: false,
        inner: depth > 0,
        written,
    }));
    for (inner, ks, chain) in &inners {
        // What it runs after moving to another directory (`uv --directory
        // ~/notes run cat ../x`) runs THERE, where this reader does not
        // follow: unsound, not placed at the start (review 2026-10-10, E1).
        if r.chdir.is_some() {
            acts.push(Act::Unsound(
                "it moves to another directory before running a command the gate reads \
                 where it started"
                    .into(),
            ));
            break;
        }
        // Nested past reading (`git git git …` eight thousand deep aborted
        // the process — review 2026-10-10, B): unsound, not read.
        if depth >= MAX_INNER_DEPTH {
            acts.push(Act::Unsound(
                "its argument lists nest deeper than the gate reads".into(),
            ));
            break;
        }
        // `{}` stands for what the search matched: under its starting
        // points — written where the `{}` is.
        let (inner, inner_at): (Vec<String>, Vec<usize>) = inner
            .iter()
            .zip(ks)
            .flat_map(|(w, &k)| {
                let written = at.get(k).copied().unwrap_or(usize::MAX);
                match (&model.placeholder, w.as_str()) {
                    (Some(p), x) if x == p => dirs
                        .iter()
                        .map(|d| (d.clone(), written))
                        .collect::<Vec<_>>(),
                    _ => vec![(w.clone(), written)],
                }
            })
            .unzip();
        if inner.is_empty() {
            continue;
        }
        let inner_live = |w: &str| live(w);
        acts.extend(acts_of_argv(
            stage,
            cx,
            &inner,
            &inner_at,
            &inner_live,
            Some((base.clone(), chain.clone())),
            depth + 1,
        ));
    }
    acts
}

/// `model` read the other way where implementations differ: each
/// `or_attached` flag's value attached only. `None` when no flag differs.
/// (Depth: a model's tree is at most `MOST_CHAIN_WORDS` deep.)
fn attached_reading(model: &Model) -> Option<Model> {
    fn walk(node: &mut Node) -> bool {
        let mut any = false;
        for f in node.flags.iter_mut().filter(|f| f.or_attached) {
            f.arity = Arity::OptionalAttached;
            f.or_attached = false;
            any = true;
        }
        for s in &mut node.subs {
            any |= walk(s);
        }
        any
    }
    let mut alt = model.clone();
    walk(&mut alt.root).then_some(alt)
}

/// The acts of `w`, a value `model` reads as code: what its language reader
/// sees in it (`sed`, `awk`), else code it does not read. The flag: the code
/// writes a file, so the program is a writer.
fn code_acts(stage: &Stage, cx: &Context, model: &Model, w: &str, base: &str) -> (Vec<Act>, bool) {
    let found = match model.language.as_deref() {
        Some("sed") => lang::sed::scan(w),
        Some("awk") => lang::awk::scan(w),
        _ => vec![lang::Found {
            role: Role::Code,
            what: "code the model does not read".into(),
        }],
    };
    let mut acts = Vec::new();
    let mut writes = false;
    for f in found {
        match f.role {
            Role::WriteFile | Role::ReadFile if f.what == DYNAMIC => {
                acts.push(Act::Unplaced(
                    "a file named by the code's data".into(),
                    Vec::new(),
                ));
            }
            Role::WriteFile => {
                writes = true;
                acts.extend(path_act(
                    stage,
                    cx,
                    &f.what,
                    Some(Effect::Write),
                    base,
                    false,
                ))
            }
            Role::ReadFile => acts.extend(path_act(
                stage,
                cx,
                &f.what,
                Some(Effect::Read),
                base,
                false,
            )),
            _ => acts.push(Act::Code {
                program: base.to_string(),
                what: f.what,
                at: Vec::new(),
            }),
        }
    }
    (acts, writes)
}

/// The most letters of a short cluster the gate splits: a person writes a
/// handful (`-xvzf`); each split is a place, so the work is bounded here
/// and a longer cluster is unplaced — it asks — rather than read in part.
pub(crate) const MAX_CLUSTER: usize = 32;

/// **Every value a dashed word of an argv no model reads may carry**: after
/// `=` (`--out=a`, `-o=a`); and for a short cluster (`-xoout.txt`), what
/// follows each of its leading letters — any one may be the flag that takes
/// the rest (`-x -o out.txt`, `-x oout.txt`), and getopt's reading is the
/// program's, not the gate's. `None`: a cluster of more than
/// [`MAX_CLUSTER`] letters. The one reading of a dashed word's values, for
/// placing them and for a deny's contact (`decide`).
pub(crate) fn cluster_values(w: &str) -> Option<Vec<String>> {
    if let Some((_, v)) = w.split_once('=') {
        return Some(vec![v.to_string()]);
    }
    let Some(body) = w.strip_prefix('-').filter(|b| !b.starts_with('-')) else {
        return Some(Vec::new());
    };
    // The letters a flag may be: up to the first character no option
    // letter is (a value may begin there, or earlier).
    let letters = body
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_alphanumeric() || *c == '#')
        .count();
    if letters > MAX_CLUSTER {
        return None;
    }
    // After the k-th letter (k ≥ 1): byte offsets, since the run is ASCII.
    Some(
        (1..=letters.min(body.len()))
            .map(|k| body[k..].to_string())
            .filter(|v| !v.is_empty())
            .collect(),
    )
}

/// A dashed word of an argv no model reads, as a flag and its value: after
/// `=`, or — a short flag with a path attached (`cp -t/etc`, `make
/// -C./sub`) — from the first `/` or `./`.
fn flag_and_value(w: &str) -> (String, Option<String>) {
    match w.split_once('=') {
        Some((n, v)) => (n.to_string(), Some(v.to_string())),
        None => match attached_path(w) {
            Some(at) => (w[..at].to_string(), Some(w[at..].to_string())),
            None => (w.to_string(), None),
        },
    }
}

/// **The places a word of an argv no model reads may name** — every plain
/// word, a dashed word's value, an assignment-shaped word's value — at no
/// operation, and the host of a URL. The reading of an unmodelled program,
/// and the floor under a completion model's (whose roles are a guess, and
/// may call a file data).
fn unmodelled_places(
    stage: &Stage,
    cx: &Context,
    w: &str,
    base: &str,
    live: &dyn Fn(&str) -> bool,
) -> Vec<Act> {
    let host = |w: &str| {
        host_of(w).map(|h| Act::Host {
            dir: Dir::Fetch,
            host: h,
            program: base.to_string(),
            at: Vec::new(),
        })
    };
    let mut acts = Vec::new();
    if w.starts_with('-') && w != "-" {
        // Every value the word may carry is a place: no model says which
        // reading runs, so each is placed (review 2026-10-11, E1: `--config=
        // .env` and `-c.env` walked past the credential guard while `--config
        // .env` asked — a value was placed only if it held a `/`).
        let values = match cluster_values(w) {
            Some(values) => values,
            None => {
                acts.push(Act::Unplaced(
                    format!(
                        "`{w}`: a cluster of more than {MAX_CLUSTER} letters, longer than the gate reads"
                    ),
                    Vec::new(),
                ));
                return acts;
            }
        };
        for v in values.iter().filter(|v| !v.is_empty()) {
            if is_url(v) {
                acts.extend(host(v));
            } else {
                acts.extend(path_act(stage, cx, v, None, base, live(v)));
            }
        }
    } else if is_url(w) {
        acts.extend(host(w));
    } else {
        acts.extend(path_act(stage, cx, w, None, base, live(w)));
        // `if=~/x`, `KEY=/x`: bash expands a `~` after the `=` of an
        // assignment-shaped argument, and the value is the file the program
        // reads (review 2026-10-10, E).
        if let Some((name, value)) = w.split_once('=')
            && !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && (value.starts_with('~') || value.starts_with('/') || value.starts_with("./"))
        {
            acts.extend(path_act(stage, cx, value, None, base, live(value)));
        }
    }
    acts
}

/// **A model's examples, held to the reader**: each `match` line must read
/// in full — its own program act knows every flag and operand — and each
/// `not_match` line must leave something unknown. Returns a sentence per
/// line that does not, for the model's author: a model that misreads its
/// own examples misreads commands. `models` are the others in force; the
/// model is read as one of them.
pub fn example_misreads(model: &Model, models: &Models) -> Vec<String> {
    let mut with = models.clone();
    with.insert(model.clone());
    let env = BTreeMap::new();
    let cx = Context {
        start: "/",
        home: "/",
        models: &with,
        env: &env,
    };
    // `None`: the line is not one command of this program the reader saw.
    let reads_in_full = |line: &str| -> Option<bool> {
        let pipeline = crate::shell::parse_line(line).ok()?;
        let [stage] = &pipeline.stages[..] else {
            return None;
        };
        acts_of(stage, &cx).into_iter().find_map(|a| match a {
            Act::Program(p) if p.name == model.name && p.via.is_none() => {
                Some(p.unknown_positionals.is_empty() && p.touches.iter().all(|t| !t.unknown))
            }
            _ => None,
        })
    };
    let mut out = Vec::new();
    for (line, want) in model
        .examples
        .matches
        .iter()
        .map(|l| (l, true))
        .chain(model.examples.not_matches.iter().map(|l| (l, false)))
    {
        let shown = super::model::clip(line);
        match reads_in_full(line) {
            None => out.push(format!(
                "example `{shown}` is not one command of `{}` the reader can see",
                model.name
            )),
            Some(full) if full != want => out.push(if want {
                format!("example `{shown}` should read in full, and a word in it is unknown")
            } else {
                format!("example `{shown}` should leave a word unknown, and every word is known")
            }),
            Some(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::parse_line;

    const START: &str = "/work/me/proj";
    const HOME: &str = "/work/me";

    fn env() -> BTreeMap<String, String> {
        [("HOME", HOME), ("TMPDIR", "/work/scratch"), ("USER", "me")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// The acts of a one-stage line, under the shipped models.
    fn acts(line: &str) -> Vec<Act> {
        let env = env();
        let cx = Context {
            start: START,
            home: HOME,
            models: Models::shipped(),
            env: &env,
        };
        let pipeline = parse_line(line).expect("parses");
        pipeline
            .stages
            .iter()
            .flat_map(|s| acts_of(s, &cx))
            .collect()
    }

    fn programs(acts: &[Act]) -> Vec<&ProgramAct> {
        acts.iter()
            .filter_map(|a| match a {
                Act::Program(p) => Some(p.as_ref()),
                _ => None,
            })
            .collect()
    }

    fn paths(acts: &[Act]) -> Vec<(Option<Effect>, String)> {
        acts.iter()
            .filter_map(|a| match a {
                Act::Path(p) => Some((p.op, p.paths.join("|"))),
                _ => None,
            })
            .collect()
    }

    fn codes(acts: &[Act]) -> Vec<&str> {
        acts.iter()
            .filter_map(|a| match a {
                Act::Code { what, .. } => Some(what.as_str()),
                _ => None,
            })
            .collect()
    }

    fn hosts(acts: &[Act]) -> Vec<(Dir, &str)> {
        acts.iter()
            .filter_map(|a| match a {
                Act::Host { dir, host, .. } => Some((*dir, host.as_str())),
                _ => None,
            })
            .collect()
    }

    fn unplaced(acts: &[Act]) -> Vec<&str> {
        acts.iter()
            .filter_map(|a| match a {
                Act::Unplaced(w, _) => Some(w.as_str()),
                _ => None,
            })
            .collect()
    }

    /// **A short flag's attached value is the flag's** for a program that
    /// does not bundle (awk), as getopt reads it: `-f/srv/x.awk` reads that
    /// file, and `-F:` is known — before, both were one unknown word, and
    /// awk's own example said otherwise.
    #[test]
    fn an_attached_short_value_is_read_without_bundling() {
        // (The program file is run: an exec, 10-13.)
        let a = acts("awk -f/srv/x.awk data.txt");
        assert!(
            paths(&a).contains(&(Some(Effect::Exec), "/srv/x.awk".into())),
            "{:?}",
            paths(&a)
        );
        for line in ["awk -f/srv/x.awk data.txt", "awk -F: '{print $1}' data.txt"] {
            let p = acts(line)
                .into_iter()
                .find_map(|a| match a {
                    Act::Program(p) => Some(p),
                    _ => None,
                })
                .unwrap();
            assert!(
                p.touches.iter().all(|t| !t.unknown),
                "{line}: {:?}",
                p.touches
            );
        }
        // A letter the model does not know stays an unknown word.
        let p = acts("awk -Zfoo '{print}' data.txt")
            .into_iter()
            .find_map(|a| match a {
                Act::Program(p) => Some(p),
                _ => None,
            })
            .unwrap();
        assert!(p.touches.iter().any(|t| t.unknown));
    }

    // ---- K: find --------------------------------------------------------------

    /// **A search is a read of its starting points, and its primaries are
    /// touched by canonical name.** `-exec` runs an inner program on what
    /// matched, which is under the starting points; `-delete` raises the
    /// act; `--` ends nothing for find.
    #[test]
    fn find_reads_its_trees_and_runs_what_exec_names() {
        let a = acts("find . -name '*.md' -type f");
        let p = programs(&a);
        assert_eq!(p.len(), 1);
        assert!(p[0].modelled && p[0].chain.is_empty());
        assert_eq!(p[0].effect, Effect::Read);
        let names: Vec<&str> = p[0].touches.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["name", "type"]);
        assert!(p[0].touches.iter().all(|t| t.benign));
        assert_eq!(paths(&a), [(Some(Effect::Read), START.to_string())]);

        let a = acts("find . -type f -exec cat {} \\;");
        let p = programs(&a);
        assert_eq!(p.len(), 2, "find, and the cat it runs");
        assert_eq!(p[0].effect, Effect::Exec);
        assert_eq!(p[1].name, "cat");
        assert_eq!(p[1].via, Some(("find".to_string(), vec![])));
        assert!(
            paths(&a)
                .iter()
                .any(|(op, p)| *op == Some(Effect::Read) && p == START),
            "`{{}}` stands for the starting points: {:?}",
            paths(&a)
        );

        let a = acts("find . -name '*.pyc' -delete");
        assert_eq!(programs(&a)[0].effect, Effect::Delete);
        let a = acts("find -- . -delete");
        assert_eq!(
            programs(&a)[0].effect,
            Effect::Delete,
            "find's `--` ends nothing"
        );
        let a = acts("find . -samefile y -newer /work/hosts");
        assert!(
            paths(&a)
                .iter()
                .any(|(op, p)| *op == Some(Effect::Read) && p == "/work/hosts")
        );
        assert!(paths(&a).iter().any(|(_, p)| p == &format!("{START}/y")));
    }

    // ---- L: gh --------------------------------------------------------------------

    /// **A subcommand is a position in the chain; a flag that opens the
    /// browser is a program run; `api`'s method is derived; an unknown
    /// first word runs an extension; a node that prints a credential needs
    /// to be named.**
    #[test]
    fn gh_reads_its_chain_and_what_its_flags_run() {
        let a = acts("gh pr view 12 --comments");
        let p = programs(&a);
        assert_eq!(p[0].chain, ["pr", "view"]);
        assert_eq!(p[0].effect, Effect::Read);
        assert!(codes(&a).is_empty());

        let a = acts("gh pr view 12 --web");
        assert_eq!(
            codes(&a).len(),
            1,
            "`--web` runs a program named by configuration"
        );
        assert_eq!(programs(&a)[0].effect, Effect::Exec);

        let a = acts("gh api -X GET user");
        let p = programs(&a);
        assert!(
            p[0].facts
                .contains(&("method".to_string(), Some("GET".to_string())))
        );
        assert!(
            p[0].facts
                .contains(&("endpoint".to_string(), Some("user".to_string())))
        );
        let call = p[0].call.as_ref().expect("the call it makes");
        assert_eq!(
            (
                call.methods.as_slice(),
                call.host.as_deref(),
                &*call.endpoint
            ),
            (&["GET".to_string()][..], Some("api.github.com"), "user")
        );
        assert!(
            call.gloss
                .as_deref()
                .is_some_and(|g| g.contains("authenticated user"))
        );
        assert!(hosts(&a).is_empty(), "the implicit host is not an act");
        // The call as the pane says it: a fetch when nothing says otherwise,
        // the method a field implies, every method given, the host named.
        let call = |line: &str| programs(&acts(line))[0].call.clone().expect("a call");
        assert_eq!(call("gh api rate_limit").methods, ["GET"]);
        assert_eq!(call("gh api repos/o/r/issues -f t=x").methods, ["POST"]);
        assert_eq!(
            call("gh api -X GET -X DELETE repos/o/r").methods,
            ["GET", "DELETE"]
        );
        assert_eq!(call("gh api repos/o/r/issues").gloss, None, "none for it");
        assert_eq!(
            call("gh api --hostname ghe.example.com user")
                .host
                .as_deref(),
            Some("ghe.example.com")
        );
        assert_eq!(programs(&acts("gh pr list"))[0].call, None, "no endpoint");

        let a = acts("gh api repos/o/r/issues -f title=x");
        assert!(
            programs(&a)[0]
                .facts
                .contains(&("method".to_string(), Some("POST".to_string()))),
            "implied"
        );
        let a = acts("gh api -X GET repos/o/r -f q=x");
        assert!(
            programs(&a)[0]
                .facts
                .contains(&("method".to_string(), Some("GET".to_string()))),
            "given beats implied"
        );

        let a = acts("gh x --frob");
        let p = programs(&a);
        assert_eq!(p.len(), 2, "gh, and the extension gh-x it runs");
        assert_eq!(p[1].name, "x");
        assert!(!p[1].modelled);

        let a = acts("gh auth token");
        assert_eq!(programs(&a)[0].needs_named, ["prints a credential"]);
        let a = acts("gh pr close 12 --delete-branch");
        assert_eq!(programs(&a)[0].effect, Effect::Delete);
    }

    // ---- M: git --------------------------------------------------------------------

    /// **A config key that names a program is a program run; `commit`
    /// without a message opens the editor; `add` writes `.git`.**
    #[test]
    fn git_names_programs_by_config_and_writes_its_index() {
        let a = acts("git -c core.pager=less log");
        assert_eq!(codes(&a), ["`core.pager` names a program it runs"]);
        assert_eq!(programs(&a)[0].chain, ["log"]);
        let a = acts("git -c user.name=me log");
        assert!(codes(&a).is_empty(), "a key that is not a program");

        let a = acts("git commit");
        assert_eq!(codes(&a), ["the editor, a program named by configuration"]);
        let a = acts("git commit -m 'fix: the wrap cache'");
        assert!(codes(&a).is_empty());
        let a = acts("git commit -q -F msg.txt");
        assert!(codes(&a).is_empty());
        assert!(
            paths(&a)
                .iter()
                .any(|(op, p)| *op == Some(Effect::Read) && p == &format!("{START}/msg.txt"))
        );

        let a = acts("git add src/main.rs");
        assert!(
            paths(&a)
                .iter()
                .any(|(op, p)| *op == Some(Effect::Write) && p == &format!("{START}/.git"))
        );
        let a = acts("git log --oneline -8");
        let p = programs(&a);
        assert!(
            p[0].facts
                .contains(&("max-count".to_string(), Some("8".to_string()))),
            "`-8` is `-n 8`: {:?}",
            p[0].facts
        );
        let a = acts("git push --force origin main");
        assert_eq!(programs(&a)[0].effect, Effect::Delete);
    }

    // ---- N: sed; O: awk -------------------------------------------------------------

    /// **The script is read by the language reader: a write names its
    /// file; `-i` turns the files read into files written; printing is a
    /// read.**
    #[test]
    fn sed_and_awk_scripts_are_read() {
        let a = acts("sed -n 1,60p crates/a.rs");
        assert_eq!(programs(&a)[0].effect, Effect::Read);
        assert_eq!(
            paths(&a),
            [(Some(Effect::Read), format!("{START}/crates/a.rs"))]
        );

        let a = acts("sed -i '' 's/a/b/' src/main.rs");
        assert_eq!(programs(&a)[0].effect, Effect::Write);
        assert!(
            paths(&a)
                .iter()
                .any(|(op, p)| *op == Some(Effect::Write) && p == &format!("{START}/src/main.rs")),
            "{:?}",
            paths(&a)
        );

        let a = acts("sed '1w /work/out' f");
        assert!(
            paths(&a)
                .iter()
                .any(|(op, p)| *op == Some(Effect::Write) && p == "/work/out")
        );
        let a = acts("sed '1e' f");
        assert_eq!(codes(&a).len(), 1);

        let a = acts("awk '{print $1}' data.txt");
        assert!(codes(&a).is_empty());
        assert_eq!(programs(&a)[0].effect, Effect::Read);
        let a = acts("awk '{ system(\"id\") }' f");
        assert_eq!(codes(&a).len(), 1);
        let a = acts("awk '{ print > \"out.txt\" }' f");
        assert!(
            paths(&a)
                .iter()
                .any(|(op, p)| *op == Some(Effect::Write) && p == &format!("{START}/out.txt"))
        );
        let a = acts("awk '{ print > $2 }' f");
        assert!(
            !unplaced(&a).is_empty(),
            "a file named by data is placed nowhere"
        );
    }

    // ---- P: uv, curl, wrappers ------------------------------------------------------

    /// **`uv run` runs an inner argv; `tree --frozen` is a read; a fetch
    /// with a body sends; a wrapper's argv is read; a path-qualified
    /// program is an exec of a file.**
    #[test]
    fn inner_argvs_hosts_and_execs() {
        let a = acts("uv run pytest tests/");
        let p = programs(&a);
        assert_eq!(p[0].chain, ["run"]);
        assert!(p[0].takes_argv);
        assert_eq!(p[1].name, "pytest");
        assert!(!p[1].modelled);
        assert_eq!(p[1].via, Some(("uv".to_string(), vec!["run".to_string()])));

        let a = acts("uv tree --frozen");
        assert_eq!(programs(&a)[0].effect, Effect::Read);
        let a = acts("uv tree");
        assert_eq!(programs(&a)[0].effect, Effect::Write);

        let a = acts("curl -s https://api.github.com/x");
        assert_eq!(hosts(&a), [(Dir::Fetch, "api.github.com")]);
        let a = acts("curl -X POST -d @body.json https://example.com/api");
        assert_eq!(hosts(&a), [(Dir::Send, "example.com")]);
        // And the file `-d @body.json` reads — "not yet", this said, when it
        // already was (review stop 12, T8).
        assert!(
            paths(&a)
                .iter()
                .any(|(op, p)| *op == Some(Effect::Read) && p == &format!("{START}/body.json")),
            "{:?}",
            paths(&a)
        );
        let a = acts("curl -T upload.bin https://example.com/put");
        assert_eq!(hosts(&a), [(Dir::Send, "example.com")]);
        assert!(
            paths(&a)
                .iter()
                .any(|(op, p)| *op == Some(Effect::Read) && p == &format!("{START}/upload.bin"))
        );
        let a = acts("curl -sSL -o out.json https://api.github.com/x");
        assert!(
            paths(&a)
                .iter()
                .any(|(op, p)| *op == Some(Effect::Write) && p == &format!("{START}/out.json"))
        );

        let a = acts("timeout 300 python3 tools/drive/smoke.py");
        let p = programs(&a);
        assert_eq!(p[0].name, "timeout");
        assert_eq!(p[1].name, "python3");
        assert!(
            paths(&a).iter().any(|(op, p)| *op == Some(Effect::Exec)
                && p == &format!("{START}/tools/drive/smoke.py"))
        );

        let a = acts("sudo ls /etc");
        assert!(
            a.iter()
                .any(|x| matches!(x, Act::Privilege(p) if p.wrapper == "sudo"))
        );

        let a = acts("./scripts/build.sh --fast");
        assert!(
            paths(&a)
                .iter()
                .any(|(op, p)| *op == Some(Effect::Exec)
                    && p == &format!("{START}/scripts/build.sh"))
        );
        assert_eq!(
            programs(&a)[0].exec_of.as_deref(),
            Some("./scripts/build.sh")
        );
    }

    /// **An unmodelled program keeps its argv as a prefix, every non-dashed
    /// word a path at any operation.**
    #[test]
    fn an_unmodelled_program_is_a_prefix_with_placed_words() {
        let a = acts("make test CC=/tmp/cc");
        let p = programs(&a);
        assert!(!p[0].modelled);
        assert_eq!(p[0].prefix_words, ["test", "CC=/tmp/cc"]);
        assert_eq!(p[0].unknown_positionals, ["test", "CC=/tmp/cc"]);
        let ps = paths(&a);
        assert!(
            ps.iter().all(|(op, _)| op.is_none()),
            "any operation: {ps:?}"
        );
        assert!(ps.iter().any(|(_, p)| p == &format!("{START}/test")));
        let a = acts("make --frob=https://x.y/z");
        assert_eq!(hosts(&a), [(Dir::Fetch, "x.y")]);
    }

    /// **A redirection is a path act at its operation; `/dev/null` is no
    /// file; the cwd a `cd` set is where words are placed; past a `cd` that
    /// may have failed, a word is placed at every place.**
    #[test]
    fn redirects_and_places() {
        let a = acts("cargo build 2>/dev/null");
        assert!(
            paths(&a)
                .iter()
                .all(|(op, p)| *op == Some(Effect::Read) && p == START),
            "no file but where it runs: {:?}",
            paths(&a)
        );
        let a = acts("cat a > /work/out");
        assert!(paths(&a).contains(&(Some(Effect::Write), "/work/out".to_string())));
        let a = acts("cd build && rm x");
        assert!(paths(&a).contains(&(Some(Effect::Delete), format!("{START}/build/x"))));
        let a = acts("cd build; rm x");
        let ps = paths(&a);
        let rm = ps
            .iter()
            .find(|(op, _)| *op == Some(Effect::Delete))
            .expect("the delete");
        assert!(
            rm.1.contains(&format!("{START}/build/x")) && rm.1.contains(&format!("{START}/x")),
            "{ps:?}"
        );
    }

    /// **The gate's own environment is the one expansion it knows** —
    /// `$TMPDIR/x` is placed under the scratch directory, `${XDG:-$HOME/…}`
    /// through its default; a variable the table lacks, or any variable
    /// once an earlier stage set one, stays unplaced.
    #[test]
    fn a_known_variable_is_placed() {
        let a = acts("cat \"$TMPDIR/y\"");
        assert!(
            paths(&a).contains(&(Some(Effect::Read), "/work/scratch/y".to_string())),
            "{:?}",
            paths(&a)
        );
        let a = acts("cat x > \"$TMPDIR/y\"");
        assert!(
            paths(&a).contains(&(Some(Effect::Write), "/work/scratch/y".to_string())),
            "{:?}",
            paths(&a)
        );
        // A name the gate does not know may be set by the shell (`$_`, a
        // `for` variable): its default is not read (review 2026-10-10, C).
        let a = acts("ls -la ${XDG_STATE_HOME:-$HOME/.local/state}/shannon/log/");
        assert!(
            unplaced(&a).iter().any(|w| w.contains("XDG_STATE_HOME")),
            "{a:?}"
        );
        let a = acts("cat $FOO/x");
        assert_eq!(unplaced(&a), ["$FOO/x"]);
        let a = acts("export X=1; cat $HOME/f");
        assert!(
            unplaced(&a).iter().any(|w| w.contains("$HOME")),
            "an earlier stage set a variable: {a:?}"
        );
        assert_eq!(substitute_known("$HOME", &env()).as_deref(), Some(HOME));
        assert_eq!(
            substitute_known("${USER}@$HOME", &env()).as_deref(),
            Some("me@/work/me")
        );
        assert_eq!(substitute_known("$1/$$", &env()), None);
        // A name the gate does not know may be the shell's: not read as
        // unset (review 2026-10-10, C); a name it knows empty takes the
        // default, and a shell-owned name is never the gate's own.
        assert_eq!(substitute_known("${UNSET:-x}", &env()), None);
        assert_eq!(substitute_known("${PWD}/x", &env()), None);
        assert_eq!(substitute_known("${_:-x}", &env()), None);
    }

    /// **A bundle, a numeric option, an attached value and an operator are
    /// read as the family says; an unknown flag is an unknown touch with
    /// the model's reason.**
    #[test]
    fn families_are_read_as_their_programs_read_them() {
        let a = acts("rm -rf build");
        let names: Vec<&str> = programs(&a)[0]
            .touches
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, ["recursive", "force"]);
        let a = acts("head -20 f");
        assert!(
            programs(&a)[0]
                .facts
                .contains(&("lines".to_string(), Some("20".to_string())))
        );
        let a = acts("rg --type=rust foo");
        let p = programs(&a);
        assert!(
            p[0].facts
                .contains(&("type".to_string(), Some("rust".to_string())))
        );
        assert!(!p[0].touches[0].benign, "a guarded flag does not ride");
        let a = acts("rg --frob foo");
        let t = &programs(&a)[0].touches[0];
        assert!(t.unknown && t.why.is_some());
        let a = acts("echo ----- x");
        assert!(programs(&a)[0].touches.is_empty(), "echo's dashes are text");
        let a = acts("find . \\( -name a -o -name b \\)");
        assert_eq!(
            programs(&a)[0].touches.len(),
            3,
            "operators are not operands: {:?}",
            programs(&a)[0].touches
        );
    }

    /// **The reader never panics**, whatever the words.
    #[test]
    fn the_reader_never_panics() {
        use proptest::prelude::*;
        let env = env();
        let cx = Context {
            start: START,
            home: HOME,
            models: Models::shipped(),
            env: &env,
        };
        proptest!(ProptestConfig::with_cases(300), |(line in "[ -~]{0,60}")| {
            if let Ok(pipeline) = parse_line(&line) {
                for s in &pipeline.stages {
                    let _ = acts_of(s, &cx);
                }
            }
        });
        let models = [
            "find", "gh", "git", "sed", "awk", "uv", "curl", "cargo", "rg", "fd", "xargs",
            "timeout", "rm",
        ];
        proptest!(ProptestConfig::with_cases(300), |(m in 0..models.len(), words in proptest::collection::vec("[ -~]{0,10}", 0..8))| {
            let line = format!("{} {}", models[m], words.iter().map(|w| format!("'{}'", w.replace('\'', ""))).collect::<Vec<_>>().join(" "));
            if let Ok(pipeline) = parse_line(&line) {
                for s in &pipeline.stages {
                    let _ = acts_of(s, &cx);
                }
            }
        });
    }
}
