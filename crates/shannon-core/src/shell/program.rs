//! Per-program knowledge, **read from the model** (`permission::model`):
//! which spellings mean which flag, whether a flag takes a value, what a
//! program (or a flag on it) can *do* — and the shell's own hazards: the
//! environment variables that change what a name runs, and the keywords of
//! control flow.
//!
//! Why this is data, and per-program: **there is no universal argv grammar.**
//! POSIX bundles shorts (`rm -rf` == `rm -r -f`); GNU adds longs (`--force`);
//! `find` uses single-dash *words* (`-name`, `-delete`) that must NOT bundle;
//! BSD `ps aux` drops the dash entirely; `dd if=… of=…` is key=value. A parser
//! that guesses a single convention is wrong for most programs. So each
//! program we understand carries its own model, and a program we do *not*
//! model is treated conservatively (see [`super::Effect::Unknown`]) rather
//! than guessed.
//!
//! **Until 2026-10-09 the model was a table in this file** (`PROGRAMS`, with
//! eight side tables: which flags name files, which run code, what the
//! operands are, …). It is now the TOML models under
//! `crates/shannon-core/models/`, one file per program, the same format a
//! user writes (`permission::model`); this module is the FLAT VIEW of them
//! that today's normalizer ([`super::normalize`]) and engine read — a
//! program as a list of flags — and the helper questions, each answered
//! from the model. The view hides what the flat reading cannot say: a
//! program with subcommands (`gh`, `git`, `cargo`, `uv`), one whose first
//! operand is code a language reader scans (`sed`, `awk`), one whose words
//! are URLs or hosts (`curl`), and a flag the model guards (`rg --hidden`,
//! `find -files0-from`) — those stay unknown to this view, which asks, as
//! they did before. The engine rebuilt around acts (P4) reads the full
//! model and this view goes with it.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use super::Effect;
use crate::permission::model::{Arity, At, Model, Models, Role};

/// One flag a program understands, as the flat view reads it.
pub struct Arg {
    /// Canonical, dashless identity a rule matches — e.g. `"delete"`. Every
    /// [`spelling`](Arg::spellings) collapses to this, so a rule about a flag
    /// holds regardless of how the flag was written.
    pub canonical: &'static str,
    /// Every spelling that means this flag, *with* dashes: shorts and longs,
    /// order-independent — e.g. `&["-r", "-R", "--recursive"]`.
    pub spellings: &'static [&'static str],
    /// Whether the flag consumes the following token as its value
    /// (`--output x`), so the value is not mistaken for an operand. (A flag
    /// that consumes *several* tokens, like `find -exec … ;`, is marked
    /// value-taking here and under-consumes — safe, because its danger is
    /// already captured by [`elevates`](Arg::elevates), not by the value.)
    pub takes_value: bool,
    /// If present, this flag raises the command to at least this effect — the
    /// "`find` is only dangerous once `-exec`/`-delete` appears" mechanism.
    /// `None` means the flag does not change what the command can do.
    pub elevates: Option<Effect>,
}

/// What the flat view says about one program.
pub struct Program {
    /// The program's basename (`rm`, not `/bin/rm`). [`lookup`] matches on the
    /// basename so an absolute or relative path resolves to the same model.
    pub name: &'static str,
    /// The effect of invoking it with no [elevating](Arg::elevates) flags.
    pub base: Effect,
    /// The flags we model. A dashed token not found here is kept as `unknown`
    /// (never dropped, never guessed) — see [`super::Command`].
    pub args: &'static [Arg],
    /// Whether single-dash shorts bundle: `-rf` ≡ `-r -f` (POSIX/GNU) vs. must
    /// stay whole (`find -name`, `-delete`). Absent a model we do **not**
    /// bundle — inventing `-n -a -m -e` out of `-name` would be worse than
    /// leaving the token whole.
    pub bundles_shorts: bool,
    /// Whether `--` ends this program's flags (POSIX's rule) — not for
    /// `find`, whose `--` ends only the options before the paths, every
    /// dashed word after it still a primary (`find -- . -delete` deletes).
    dashdash_ends: bool,
}

impl Program {
    /// The flags that raise this program ABOVE its base effect — what a hidden
    /// flag could be ([`can_escalate`]) and what the default rules have to
    /// carve out beneath a broad allow (`permission::defaults`).
    pub fn escalations(&self) -> impl Iterator<Item = &Arg> {
        self.args
            .iter()
            .filter(|a| a.elevates.is_some_and(|e| e.rank() > self.base.rank()))
    }

    /// **Whether `--` ends this program's flags** — POSIX's rule, and every
    /// modelled program's but `find`'s. Read as the end of the flags, it hid
    /// every primary from the gate — `find -- . -exec rm -rf {} +` ran
    /// unattended under the shipped defaults (review 28, Reviewer E2). A
    /// program's grammar, so a fact of its model (`family.double_dash`).
    pub fn dashdash_ends_flags(&self) -> bool {
        self.dashdash_ends
    }
}

/// The flat views, built once from the shipped models. The slices are
/// leaked ONCE here, bounded by the shipped set, so the view can hand out
/// `&'static` like the table it replaced; the strings themselves are the
/// static models', not copies.
fn flat() -> &'static BTreeMap<&'static str, Program> {
    static FLAT: OnceLock<BTreeMap<&'static str, Program>> = OnceLock::new();
    FLAT.get_or_init(|| {
        Models::shipped()
            .iter()
            .filter_map(|m| flat_view(m).map(|p| (m.name.as_str(), p)))
            .collect()
    })
}

/// The flat view of one model, or `None` where the flat reading would be
/// wrong: a tree (subcommands, or an unknown word that runs a program), a
/// word that is a URL or a host. A program whose operand is code (`awk`,
/// `sed`) is presented as an EXECUTOR: the flat reading cannot scan the
/// code, so it is what the table called it — a program that runs
/// arbitrary code — until the acts engine reads the language. A flag the
/// model guards (`benign = false`) is left out, so the normalizer keeps it
/// `unknown` and the engine asks with the model's reason
/// ([`guarded_mode`]).
fn flat_view(model: &'static Model) -> Option<Program> {
    let root = &model.root;
    if !root.subs.is_empty() || root.external {
        return None;
    }
    let unreadable = |r: Role| matches!(r, Role::Url | Role::Host | Role::Endpoint);
    if root.positionals.iter().any(|p| unreadable(p.role))
        || root.flags.iter().any(|f| unreadable(f.role))
    {
        return None;
    }
    let runs_code_it_cannot_read = root.positionals.iter().any(|p| p.role == Role::Code);
    let args: Vec<Arg> = root
        .flags
        .iter()
        .filter(|f| f.benign)
        .map(|f| Arg {
            canonical: f.canon.as_str(),
            spellings: Box::leak(
                f.names
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<&'static str>>()
                    .into_boxed_slice(),
            ),
            takes_value: !matches!(f.arity, Arity::Zero),
            elevates: f.effect.map(crate::permission::model::Effect::shown),
        })
        .collect();
    Some(Program {
        name: model.name.as_str(),
        base: if runs_code_it_cannot_read {
            Effect::Executor
        } else {
            root.effect.shown()
        },
        args: Box::leak(args.into_boxed_slice()),
        bundles_shorts: model.family.bundling,
        dashdash_ends: model.family.double_dash != crate::permission::model::DoubleDash::Ignored,
    })
}

/// Every program the flat view can say — what today's normalizer and
/// engine know as "modelled". The tests' walk over the whole set.
#[cfg(test)]
pub(crate) fn flat_programs() -> impl Iterator<Item = &'static Program> {
    flat().values()
}

/// The basename a program is known by.
fn base_of(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// The shipped model for `name`, by basename.
fn model_of(name: &str) -> Option<&'static Model> {
    Models::shipped().get(name)
}

/// Look a program up by name, matching on the **basename** so `/usr/bin/rm`,
/// `./rm`, and `rm` resolve to the same model. Returns `None` for a program
/// the flat view cannot say — the caller must then treat it conservatively
/// (fail closed), never assume it is safe.
///
/// Not yet handled (documented gaps, not silent ones): `PATH` resolution —
/// we match the written name, we do not resolve what would actually run.
pub fn lookup(name: &str) -> Option<&'static Program> {
    flat().get(base_of(name))
}

/// The canonical name `program`'s model gives the flag spelled `spelling`
/// (`rm`, `-R` → `recursive`), or `None` when it models no such flag.
///
/// One resolver for a rule's flag, whenever it is read: when the rule is
/// parsed against the program it names, and when a `*` rule — which names
/// none — meets a command (`permission::rule`). Read from the FLAT view,
/// not the full model: a flag resolved to a canonical name is matched
/// against the flags the normalizer read, and for a program the view does
/// not present (a tree: `gh`) or a flag it leaves out (a guarded `rg
/// --hidden`) the normalizer reads none — so such a flag stays literal and
/// is matched as written, as the table had it (`deny gh api -X` fires on
/// `-X`; the as-written note says so).
pub fn canonical_of(program: &str, spelling: &str) -> Option<&'static str> {
    lookup(program)?
        .args
        .iter()
        .find(|a| a.spellings.contains(&spelling))
        .map(|a| a.canonical)
}

/// Could a *flag* make this program more dangerous than its base effect?
///
/// The question an unresolved operand raises: `find . "$X"` matters because `$X`
/// could expand to `-delete`, but `echo "$X"` cannot become anything — `echo`
/// has no flag that elevates. Distinguishing the two keeps the "an unresolved
/// value is hiding something" warning on the commands where it could be true.
/// An unmodeled program is assumed escalatable (fail closed). A guarded flag
/// counts: a flag we cannot see could be that one.
pub fn can_escalate(name: &str) -> bool {
    Models::shipped().can_escalate(name)
}

/// **Does this program's `canonical` flag take a variable NAME as its
/// value**, rather than data? `read -a PATH` writes `PATH`; `read -d PATH
/// VAR` uses it as a delimiter and writes `VAR`. Only the first disturbs
/// name resolution.
///
/// **A seed, not the list** — the same posture as the credential shapes. It
/// exists as a lookup rather than a condition at the one call site that
/// needs it today, because the next caller would otherwise re-derive it and
/// get it differently (`[[feedback_rules_rot_by_retyping]]`).
pub fn names_a_variable(program: &str, canonical: &str) -> bool {
    matches!((base_of(program), canonical), ("read", "array"))
}

/// **Does this program's `canonical` flag take a FILE as its value?** — the
/// sibling question to [`names_a_variable`], answered by the flag's role.
///
/// An `allow` scoped to paths must cover every file a command touches, and a
/// flag's value is one exactly when the flag names a file. Values in general
/// are not (`git commit -m "msg"` is not a path a grant should have to cover),
/// which is why containment skipped them all — and so `allow tar -c /tmp/*`
/// covered `tar -c --file=/etc/passwd /tmp/a`, the archive written over
/// `/etc/passwd` (SECURITY-2026-09-30 #3).
pub fn names_a_file(program: &str, canonical: &str) -> bool {
    // The model's own answer (`Role::is_file`): a hand list here missed
    // `list` when it was added (10-11).
    flag_role(program, canonical).is_some_and(Role::is_file)
}

/// The role of `program`'s `canonical` flag, anywhere in its tree.
fn flag_role(program: &str, canonical: &str) -> Option<Role> {
    model_of(program)?
        .root
        .flags_below()
        .into_iter()
        .find(|(_, f)| f.canon == canonical)
        .map(|(_, f)| f.role)
}

/// **What a program's operands ARE**, to the question "which files does it
/// read" (`permission::file_operand`). A program not modelled is
/// [`Operands::Files`], every operand a file — the reading that can only add
/// a file, never miss one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operands {
    /// Each operand is a file or a directory it reads (`cat`, `ls`).
    Files,
    /// The first operand is a pattern or a script — unless one of these flags
    /// (canonical names) gave one, or changed what the operands are (`rg
    /// --files`) — and the rest are files (`grep`, `rg`).
    PatternFirst(&'static [&'static str]),
    /// None is a file it reads: words it prints or compares, a number, a name
    /// (`echo`, `test`, `basename`) — or a place to go, which is where later
    /// commands read, not this one (`cd`).
    Data,
}

/// What `program`'s operands are, from its model's positionals: a pattern
/// first where the model says so, data where every operand is, files
/// otherwise — and files for a program no model describes.
pub fn operands(program: &str) -> Operands {
    static OPERANDS: OnceLock<BTreeMap<&'static str, Operands>> = OnceLock::new();
    let table = OPERANDS.get_or_init(|| {
        Models::shipped()
            .iter()
            .map(|m| {
                let ps = &m.root.positionals;
                let first = ps.iter().find(|p| p.at == At::Index(0));
                let what = match first {
                    Some(p) if p.role == Role::Pattern => Operands::PatternFirst(Box::leak(
                        p.unless
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<&'static str>>()
                            .into_boxed_slice(),
                    )),
                    _ if !ps.is_empty()
                        && ps.iter().all(|p| {
                            matches!(p.role, Role::Data | Role::Number | Role::Enum | Role::Argv)
                        }) =>
                    {
                        Operands::Data
                    }
                    _ => Operands::Files,
                };
                (m.name.as_str(), what)
            })
            .collect()
    });
    table
        .get(base_of(program))
        .copied()
        .unwrap_or(Operands::Files)
}

/// Does `program` read where it runs, given no file? — `ls` lists it, `rg`
/// and `find` search it. `cat` with none reads its input instead, which is
/// not a place.
pub fn reads_here(program: &str) -> bool {
    model_of(program).is_some_and(|m| m.root.reads_here.is_some())
}

/// **Does this program's `canonical` flag take CODE as its value?** —
/// `bash -c '…'`, `python3 -c …`, `find -exec …`: a program run, or code.
///
/// A grant scoped to paths covers files, and code is not a file it can
/// cover: `allow bash build.sh` covered `bash -c 'touch /tmp/pwned'
/// build.sh`, the script it named standing beside code it never saw
/// (SECURITY-2026-09-30 R2-4). Such a grant covers the command only if it
/// names the flag itself.
pub fn runs_code(program: &str, canonical: &str) -> bool {
    model_of(program).is_some_and(|m| {
        m.root.flags_below().into_iter().any(|(_, f)| {
            f.canon == canonical
                && match f.role {
                    // Code text, or a command line of several words
                    // (`find -exec CMD ARGS ;`); a flag that names ONE program
                    // (`rg --pre PROG`) is a program run, which the flat
                    // view's engine carves by name instead.
                    Role::Code | Role::Argv => true,
                    Role::Program => matches!(f.arity, Arity::Until(_) | Arity::N(_)),
                    _ => false,
                }
        })
    })
}

/// **What `-NUM` means**, for the programs where a dashed number is a flag
/// with its value: `head -20` is `head -n 20`, `grep -2` is `grep -C 2`. A
/// dashed number otherwise matches no spelling and asks, which put a prompt
/// on `ls -la | head -20` (the 2026-09-25 demo).
pub fn dash_number(program: &str) -> Option<&'static str> {
    model_of(program)?.family.numeric_option.as_deref()
}

/// **Programs whose dash-led words are DATA** unless they are the program's
/// own flags: `echo "---"` prints three dashes, and `[ -f x ]`'s `-f` is part
/// of an expression. Anywhere else a dashed word the model does not know is a
/// caution signal (`unknown`, which asks) — and that put a prompt on
/// `echo "---"` (the 2026-09-25 demo) and on every `[ -f … ]`.
///
/// **Explicit, never inferred** (`family.dash_data`): a program is listed
/// because what it does with such a word is known to be nothing but data.
/// Not `printf`, whose `-v VAR` WRITES a variable, and whose dashes
/// `writes_hazard_var` reads from `unknown` — listing it would hide
/// `printf -v PATH …` from that check.
pub fn dashes_are_data(program: &str) -> bool {
    model_of(program).is_some_and(|m| m.family.dash_data)
}

/// **The operand a program WRITES**, by position, for the programs that have
/// one: `uniq IN OUT` writes `OUT`. An escape hatch in an operand's place,
/// which no flag announces — so its effect is raised by position.
pub fn writes_operand(program: &str) -> Option<usize> {
    model_of(program)?
        .root
        .positionals
        .iter()
        .find_map(|p| match (p.at, p.role) {
            (At::Index(i), Role::WriteFile) => Some(i),
            _ => None,
        })
}

/// Why one of `program`'s `flags` asks for its value, if one does: an
/// include glob that is not a `!` exclude (`rg -g '**'` reads every `.env`
/// and key under home — the model's `include_glob` role), or a guarded flag
/// the flat view let through for its value (none today).
pub fn guarded_value(program: &str, flags: &[super::Flag]) -> Option<&'static str> {
    let model = model_of(program)?;
    flags.iter().find_map(|given| {
        let spec = model.root.flags.iter().find(|f| f.canon == given.name)?;
        let value = given.value.as_deref()?;
        let reaches = match spec.role {
            Role::IncludeGlob => !value.starts_with('!'),
            _ => !spec.benign,
        };
        if reaches { spec.why.as_deref() } else { None }
    })
}

/// Why `name`'s unmodelled flags ask, for a program whose model leaves its
/// dangerous modes guarded (`unknown_flag_why`: `grep -r`, `rg --hidden`).
pub fn guarded_mode(name: &str) -> Option<&'static str> {
    model_of(name)?.unknown_flag_why.as_deref()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrap {
    /// The inner command follows the wrapper's own flags: `sudo -u me rm …`,
    /// `nice rm …`, `env FOO=1 rm …`.
    AfterFlags,
    /// ...and one positional argument first: `timeout 5 rm …`, `flock f rm …`.
    AfterFlagsAndOneOperand,
}

/// A program whose arguments *are another command*.
///
/// `sudo rm -rf /` is an `rm`, not a `sudo` — so a rule about `rm` must match it.
/// Without unwrapping, every wrapper is a one-token bypass of every name-based
/// rule, which is the same defect as an env-var prefix hiding the program.
///
/// Note the deliberate omission: `sh -c '…'`, `su -c '…'` and friends carry the
/// inner command as a *quoted string* that would have to be re-lexed. We do not
/// unwrap those — they stay [`Effect::Executor`], which is honest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wrapper {
    pub name: &'static str,
    pub kind: Wrap,
}

/// The wrapper model for a program, if it is one: the model's operands are
/// another command line (`argv`), after its flags — and after one operand
/// first, where the model says so (`timeout 5 cmd`, `flock f cmd`).
pub fn wrapper(name: &str) -> Option<Wrapper> {
    let model = model_of(name)?;
    let ps = &model.root.positionals;
    let argv = ps.iter().any(|p| p.at == At::Rest && p.role == Role::Argv);
    if !argv {
        return None;
    }
    let kind = if ps.iter().any(|p| p.at == At::Index(0)) {
        Wrap::AfterFlagsAndOneOperand
    } else {
        Wrap::AfterFlags
    };
    Some(Wrapper {
        name: model.name.as_str(),
        kind,
    })
}

/// Shell control-flow keywords. We do not model control flow — a loop body runs
/// an unknown number of times over values we cannot know — so a line containing
/// one is refused rather than analyzed as if the keyword were a program.
pub const KEYWORDS: &[&str] = &[
    "for", "while", "until", "do", "done", "if", "then", "elif", "else", "fi", "case", "esac",
    "select", "function", "coproc", "{", "}", "[[", "]]", "!",
];

/// The control-flow keyword this word is, if any.
pub fn keyword(word: &str) -> Option<&'static str> {
    KEYWORDS.iter().copied().find(|k| *k == word)
}

/// An environment variable that changes what a command *is* or what it can do.
///
/// `LD_PRELOAD=/evil.so ls` is not "an `ls`" — it is arbitrary code wearing
/// `ls`'s name. These are the environment's escape hatches, exactly analogous to
/// `find -exec`, and the same discipline applies: the list must be honest about
/// what it covers, and it grows.
pub struct EnvHazard {
    /// Variable name, or a prefix ending in `*` (e.g. `DYLD_*`).
    pub name: &'static str,
    /// What setting this variable makes the command.
    pub effect: Effect,
    /// Plain-language reason, shown in the approval prompt and by `hints` —
    /// the answer to "why am I being asked about a variable?"
    pub why: &'static str,
}

/// The hazard for a variable name, honoring `PREFIX_*` entries.
pub fn env_hazard(name: &str) -> Option<&'static EnvHazard> {
    ENV_HAZARDS.iter().find(|h| match h.name.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => h.name == name,
    })
}

/// Variables that inject code or redirect name resolution. Seeded from the
/// well-known set (dynamic-loader overrides, interpreter options, and the
/// "run this command for me" hooks various tools expose).
pub const ENV_HAZARDS: &[EnvHazard] = &[
    // Identity: the name no longer tells you which program runs.
    EnvHazard {
        name: "PATH",
        effect: Effect::Unknown,
        why: "changes which program a name resolves to, so this may not be the program it names",
    },
    // Dynamic-loader injection: runs code inside *any* program.
    EnvHazard {
        name: "LD_PRELOAD",
        effect: Effect::Executor,
        why: "loads an arbitrary library into the program before it starts",
    },
    EnvHazard {
        name: "LD_LIBRARY_PATH",
        effect: Effect::Executor,
        why: "changes which libraries the program loads",
    },
    EnvHazard {
        name: "LD_AUDIT",
        effect: Effect::Executor,
        why: "loads an auditing library that runs alongside the program",
    },
    EnvHazard {
        name: "DYLD_*",
        effect: Effect::Executor,
        why: "macOS dynamic-loader override: can inject code into the program",
    },
    // Shell startup and word splitting.
    EnvHazard {
        name: "BASH_ENV",
        effect: Effect::Executor,
        why: "runs a script before a non-interactive shell starts",
    },
    EnvHazard {
        name: "ENV",
        effect: Effect::Executor,
        why: "runs a script when a shell starts",
    },
    EnvHazard {
        name: "IFS",
        // NOT `Executor`. `LD_PRELOAD` injects code into any program; `IFS`
        // only changes how *unquoted expansions* split into words — the command
        // still runs the program it names. Treating them alike painted the
        // single most-recommended safe idiom (`IFS= read -r line`, which sets it
        // EMPTY precisely to stop splitting) in "arbitrary code execution" red,
        // and a red that fires on best practice is a red people learn to ignore.
        // `Unknown` is the honest reading: what this command receives may differ
        // from what it looks like. It still asks; it just doesn't shout.
        //
        // The genuinely dangerous case — IFS set *persistently*, poisoning later
        // stages — is caught on the separate BINDINGS axis
        // (`pipeline::writes_hazard_var`), which is where it belongs.
        effect: Effect::Unknown,
        why: "changes how the shell splits words, so a later command may receive \
              different arguments than it appears to",
    },
    // Interpreter options: all can load or run arbitrary code.
    EnvHazard {
        name: "NODE_OPTIONS",
        effect: Effect::Executor,
        why: "passes options to node, which can require arbitrary modules",
    },
    EnvHazard {
        name: "PYTHONPATH",
        effect: Effect::Executor,
        why: "changes which Python modules are imported",
    },
    EnvHazard {
        name: "PYTHONSTARTUP",
        effect: Effect::Executor,
        why: "runs a Python file when the interpreter starts",
    },
    EnvHazard {
        name: "PERL5LIB",
        effect: Effect::Executor,
        why: "changes which Perl modules load",
    },
    EnvHazard {
        name: "PERL5OPT",
        effect: Effect::Executor,
        why: "passes options to perl, which can run code",
    },
    EnvHazard {
        name: "RUBYOPT",
        effect: Effect::Executor,
        why: "passes options to ruby, which can run code",
    },
    // "Run this command for me" hooks.
    EnvHazard {
        name: "GIT_SSH_COMMAND",
        effect: Effect::Executor,
        why: "git runs this command instead of ssh",
    },
    EnvHazard {
        name: "GIT_EXTERNAL_DIFF",
        effect: Effect::Executor,
        why: "git runs this command to produce diffs",
    },
    EnvHazard {
        name: "EDITOR",
        effect: Effect::Executor,
        why: "programs launch this command as an editor",
    },
    EnvHazard {
        name: "VISUAL",
        effect: Effect::Executor,
        why: "programs launch this command as an editor",
    },
    EnvHazard {
        name: "RIPGREP_CONFIG_PATH",
        effect: Effect::Executor,
        why: "rg reads its flags from this file, and one (--pre) runs a program",
    },
    EnvHazard {
        name: "PAGER",
        effect: Effect::Executor,
        why: "programs launch this command to page output",
    },
    // Toolchain flags that reach the linker, which can run code.
    EnvHazard {
        name: "RUSTFLAGS",
        effect: Effect::Executor,
        why: "passes flags to the compiler and linker, which can run code",
    },
    EnvHazard {
        name: "GOFLAGS",
        effect: Effect::Executor,
        why: "passes flags to the go toolchain, which can run code",
    },
    // More "run this command for me" hooks and shell-startup vectors (Reviewer E).
    EnvHazard {
        name: "PROMPT_COMMAND",
        effect: Effect::Executor,
        why: "bash runs this command before each prompt",
    },
    EnvHazard {
        name: "PS4",
        effect: Effect::Executor,
        why: "expanded (and can run commands) when xtrace is on",
    },
    EnvHazard {
        name: "BASH_FUNC_*",
        effect: Effect::Executor,
        why: "imports a shell function into the environment (Shellshock-style)",
    },
    EnvHazard {
        name: "LESSOPEN",
        effect: Effect::Executor,
        why: "less runs this command to preprocess input",
    },
    EnvHazard {
        name: "LESSCLOSE",
        effect: Effect::Executor,
        why: "less runs this command after viewing",
    },
    EnvHazard {
        name: "GIT_PAGER",
        effect: Effect::Executor,
        why: "git runs this command to page output",
    },
    EnvHazard {
        name: "MANPAGER",
        effect: Effect::Executor,
        why: "man runs this command to page output",
    },
    EnvHazard {
        name: "GIT_EDITOR",
        effect: Effect::Executor,
        why: "git launches this command as an editor",
    },
    // **The rest of git's family**, enumerated whole once one member was known
    // (Reviewer E, 2026-09-30): the four above were listed and `GIT_CONFIG*`,
    // which sets ALL of them from a file, was not — so `export
    // GIT_CONFIG_GLOBAL=./e; git status` ran under `allow git *`. Each name is
    // in the local git(1), git-config(1) or git-init(1); the prefix covers
    // `GIT_CONFIG_{GLOBAL,SYSTEM,COUNT,KEY_n,VALUE_n,…}`.
    EnvHazard {
        name: "GIT_CONFIG*",
        effect: Effect::Executor,
        why: "git reads configuration from here, and configuration can run commands",
    },
    EnvHazard {
        name: "GIT_DIR",
        effect: Effect::Executor,
        why: "git uses this repository, and its configuration can run commands",
    },
    EnvHazard {
        name: "GIT_EXEC_PATH",
        effect: Effect::Executor,
        why: "git runs its subcommands from this directory",
    },
    EnvHazard {
        name: "GIT_TEMPLATE_DIR",
        effect: Effect::Executor,
        why: "git copies hooks from here into a new repository",
    },
    EnvHazard {
        name: "GIT_SEQUENCE_EDITOR",
        effect: Effect::Executor,
        why: "git launches this command to edit a rebase",
    },
    EnvHazard {
        name: "GIT_SSH",
        effect: Effect::Executor,
        why: "git runs this program instead of ssh",
    },
    EnvHazard {
        name: "GIT_PROXY_COMMAND",
        effect: Effect::Executor,
        why: "git runs this command to connect",
    },
    EnvHazard {
        name: "GIT_ASKPASS",
        effect: Effect::Executor,
        why: "git runs this program to ask for a password",
    },
    EnvHazard {
        name: "SSH_ASKPASS",
        effect: Effect::Executor,
        why: "ssh runs this program to ask for a passphrase",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every spelling the flat view hands out is well-formed and no canonical
    /// name is empty — the loader refuses both, and this holds the view to
    /// the loader.
    #[test]
    fn the_flat_view_is_well_formed() {
        let mut programs = 0;
        for p in flat_programs() {
            programs += 1;
            assert!(!p.name.is_empty(), "program with empty name");
            for a in p.args {
                assert!(!a.canonical.is_empty(), "{}: empty canonical", p.name);
                assert!(
                    !a.spellings.is_empty(),
                    "{}: {} has no spellings",
                    p.name,
                    a.canonical
                );
                for s in a.spellings {
                    assert!(
                        s.starts_with('-'),
                        "{}: spelling {s:?} lacks a dash",
                        p.name
                    );
                }
            }
        }
        assert!(
            programs >= 60,
            "premise: the shipped set is read ({programs})"
        );
    }

    /// **The flat view hides what the flat reading cannot say** — a tree, a
    /// code operand, a URL — and says the rest: those programs are modelled
    /// to today's engine exactly as the table made them.
    #[test]
    fn the_flat_view_hides_what_todays_engine_cannot_read() {
        for tree in ["gh", "git", "cargo", "uv"] {
            assert!(lookup(tree).is_none(), "{tree} has subcommands");
        }
        for code in ["sed", "awk"] {
            let p = lookup(code).unwrap_or_else(|| panic!("{code} is flat"));
            assert_eq!(
                p.base,
                Effect::Executor,
                "{code}'s operand is code the view cannot scan"
            );
        }
        assert!(lookup("curl").is_none(), "curl's words are URLs");
        for flat in [
            "find", "rg", "fd", "rm", "ls", "cat", "xargs", "sudo", "timeout", "python3",
        ] {
            assert!(lookup(flat).is_some(), "{flat} is flat");
        }
        // A guarded flag stays unknown to the view, and the model says why.
        let rg = lookup("rg").unwrap();
        assert!(!rg.args.iter().any(|a| a.spellings.contains(&"--hidden")));
        assert!(guarded_mode("rg").is_some());
        let find = lookup("find").unwrap();
        assert!(
            !find.args.iter().any(|a| a.canonical == "files0-from"),
            "review 28: asks as unknown"
        );
        assert!(find.args.iter().any(|a| a.canonical == "samefile"));
    }

    /// `find`'s dangerous flags must ALL be in the view and ALL elevate. A
    /// danger list with gaps is worse than no list — it implies a coverage it
    /// doesn't have, and the missing flag is the one that bites. (An
    /// overwrite — `-fprint`, `-fls` — is a write placed by the path it
    /// names, where the table once called it destructive.)
    #[test]
    fn every_dangerous_find_flag_elevates() {
        let find = lookup("find").expect("find is modeled");
        let expected: &[(&str, Effect)] = &[
            ("-exec", Effect::Executor),
            ("-execdir", Effect::Executor),
            ("-ok", Effect::Executor),
            ("-okdir", Effect::Executor),
            ("-delete", Effect::Destructive),
            ("-fls", Effect::Mutating),
            ("-fprint", Effect::Mutating),
            ("-fprint0", Effect::Mutating),
            ("-fprintf", Effect::Mutating),
        ];
        for (spelling, effect) in expected {
            let arg = find
                .args
                .iter()
                .find(|a| a.spellings.contains(spelling))
                .unwrap_or_else(|| panic!("find is missing {spelling}"));
            assert_eq!(
                arg.elevates,
                Some(*effect),
                "{spelling} must elevate to {effect:?}"
            );
        }
    }

    /// Aliases must be spellings of the SAME flag. Grouping genuinely different
    /// flags (`-exec` vs `-execdir`) under one canonical would make a rule about
    /// one of them impossible to express.
    #[test]
    fn distinct_find_flags_have_distinct_canonicals() {
        let find = lookup("find").expect("find is modeled");
        for pair in [
            ("-exec", "-execdir"),
            ("-ok", "-okdir"),
            ("-fls", "-fprint"),
        ] {
            let canonical = |s: &str| {
                find.args
                    .iter()
                    .find(|a| a.spellings.contains(&s))
                    .map(|a| a.canonical)
            };
            assert_ne!(
                canonical(pair.0),
                canonical(pair.1),
                "{} and {} are different flags",
                pair.0,
                pair.1
            );
        }
    }

    /// **A bundle refused for an unknown letter keeps its known letters'
    /// danger.** This was an INVARIANT — no bundling program may have a
    /// single-letter elevating short, because a refused bundle (`-xz`, `z`
    /// unknown) dropped every letter, elevating ones included (Reviewers A,
    /// E) — and it made `sort -o` impossible to model. The normalizer joins
    /// the known letters' elevation into the effect now; this checks it for
    /// every such short, taken from the model rather than listed.
    #[test]
    fn a_refused_bundle_keeps_its_known_letters_danger() {
        let mut checked = 0;
        for p in flat_programs().filter(|p| p.bundles_shorts) {
            let unknown = ["-Q", "-W", "-X", "-Y"]
                .into_iter()
                .find(|s| !p.args.iter().any(|a| a.spellings.contains(s)))
                .expect("some letter is not this program's");
            for a in p.args {
                let Some(danger) = a.elevates else { continue };
                for s in a
                    .spellings
                    .iter()
                    .filter(|s| s.len() == 2 && s.starts_with('-'))
                {
                    let bundle = format!("{unknown}{}", &s[1..]);
                    let cmd = super::super::normalize(p.name, &[bundle.clone(), "x".into()]);
                    assert!(
                        !cmd.unknown.is_empty(),
                        "{} {bundle}: premise, refused",
                        p.name
                    );
                    assert!(
                        cmd.effect.rank() >= danger.rank(),
                        "{} {bundle} hid {s}: {:?}",
                        p.name,
                        cmd.effect
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 0, "premise: `sort -o` is one");
    }

    #[test]
    fn lookup_matches_on_basename() {
        assert!(lookup("rm").is_some());
        assert_eq!(lookup("/bin/rm").map(|p| p.name), Some("rm"));
        assert_eq!(lookup("./rm").map(|p| p.name), Some("rm"));
        assert!(lookup("definitely-not-a-real-program").is_none());
    }

    /// **The helper questions read the model**: a file flag, a code flag, a
    /// wrapper, the operands, a dashed number, dashes as data, the written
    /// operand — each the fact the table once held.
    #[test]
    fn the_helpers_answer_from_the_model() {
        assert!(names_a_file("find", "newer"));
        assert!(names_a_file("sort", "output"));
        assert!(!names_a_file("git", "message"));
        assert!(runs_code("bash", "command"));
        assert!(runs_code("find", "exec"));
        assert!(!runs_code("rm", "recursive"));
        assert_eq!(wrapper("sudo").map(|w| w.kind), Some(Wrap::AfterFlags));
        assert_eq!(
            wrapper("timeout").map(|w| w.kind),
            Some(Wrap::AfterFlagsAndOneOperand)
        );
        assert_eq!(
            wrapper("flock").map(|w| w.kind),
            Some(Wrap::AfterFlagsAndOneOperand)
        );
        assert!(wrapper("ls").is_none());
        assert_eq!(
            operands("grep"),
            Operands::PatternFirst(&["regexp", "file"])
        );
        assert_eq!(operands("echo"), Operands::Data);
        assert_eq!(operands("cat"), Operands::Files);
        assert_eq!(operands("not-a-program"), Operands::Files);
        assert_eq!(dash_number("head"), Some("lines"));
        assert_eq!(dash_number("grep"), Some("context"));
        assert_eq!(dash_number("ls"), None);
        assert!(dashes_are_data("echo"));
        assert!(!dashes_are_data("printf"));
        assert_eq!(writes_operand("uniq"), Some(1));
        assert_eq!(writes_operand("cat"), None);
        assert!(reads_here("ls") && reads_here("rg") && !reads_here("cat"));
        assert_eq!(canonical_of("rm", "-R"), Some("recursive"));
        assert_eq!(
            canonical_of("rg", "--hidden"),
            None,
            "a guarded flag is outside the view: a rule naming it is matched as written"
        );
        assert_eq!(
            canonical_of("gh", "-X"),
            None,
            "a tree is outside the view: as written"
        );
        assert!(can_escalate("find") && !can_escalate("echo") && can_escalate("unknown-program"));
    }

    /// **A guarded value asks with the model's reason**: an include glob
    /// reaches what rg skips unless it excludes.
    #[test]
    fn a_guarded_value_says_why() {
        let flag = |name: &str, value: &str| super::super::Flag {
            name: name.to_string(),
            value: Some(value.to_string()),
        };
        assert!(guarded_value("rg", &[flag("glob", "**")]).is_some());
        assert!(
            guarded_value("rg", &[flag("glob", "!target/**")]).is_none(),
            "an exclude narrows"
        );
        assert!(guarded_value("rg", &[flag("line-number", "1")]).is_none());
        assert!(guarded_value("ls", &[flag("l", "1")]).is_none());
    }
}
