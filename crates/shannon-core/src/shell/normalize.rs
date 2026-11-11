//! Turning command text into a normalized [`Command`] — the two steps that make
//! a rule spelling-invariant.
//!
//! 1. [`tokenize_simple`] splits ONE simple command into words, honoring quotes,
//!    and **fails closed** on anything it will not interpret (pipes, `&&`, `;`,
//!    redirections, subshells, command substitution). It never guesses.
//! 2. [`normalize`] classifies those words against the program's
//!    [`Program`](super::program::Program) model into its flags (every
//!    occurrence, by canonical name — so a question about them is order-free),
//!    operands, and unrecognized tokens.
//!
//! Compound lines (`a && b | c`) are decomposed one layer up, in
//! [`pipeline`](super::pipeline), which reuses the same lexer and adds the
//! working-directory state model. This file stays the *simple command* step.

use std::collections::BTreeMap;

use super::lex::{Lexeme, Operator, lex};
use super::program::{Program, lookup};
use super::{Command, Effect, Flag, Unanalyzable, is_flag_word};

/// Split a **single simple command** into words, via the shared
/// [`lex`](super::lex::lex) scan. Returns [`Unanalyzable`] if the input holds
/// *any* shell operator — a simple command by definition has none, and analyzing
/// a fragment of something larger while calling it safe is exactly the mistake
/// we refuse to make. For a whole line use
/// [`parse_line`](super::pipeline::parse_line).
pub fn tokenize_simple(input: &str) -> Result<Vec<String>, Unanalyzable> {
    let mut words = Vec::new();
    for lexeme in lex(input)? {
        // Spans aren't consumed on the simple-command path; match the bare node.
        match lexeme.node {
            Lexeme::Word(w) => words.push(w),
            Lexeme::Op(op) => return Err(Unanalyzable::ShellStructure { found: op.symbol() }),
            // A substitution embeds another command; seeing *inside* it is the
            // whole-line [`parse_line`](super::pipeline::parse_line)'s job. The
            // simple-command path stays conservative and refuses.
            Lexeme::Subst(_) => return Err(Unanalyzable::Substitution),
            // Only after a heredoc operator, which returned above.
            Lexeme::HereDoc(_) => {
                return Err(Unanalyzable::ShellStructure {
                    found: Operator::HereDoc.symbol(),
                });
            }
        }
    }
    if words.is_empty() {
        return Err(Unanalyzable::Empty);
    }
    Ok(words)
}

/// Normalize a whole invocation, including any leading `NAME=VALUE` assignments.
///
/// **This is the entry point callers should use.** Treating `FOO=bar rm -rf /`
/// as a program literally named `FOO=bar` would mean a rule about `rm` never
/// matches it — an env-var prefix would silently evade every deny rule, which is
/// a known bypass class. So assignments are parsed off the front, and the real
/// program is identified.
///
/// Assignments also *elevate*: a hazardous variable
/// ([`EnvHazard`](super::program::EnvHazard)) can turn any program into an
/// arbitrary executor, so `LD_PRELOAD=/x ls` is not a read-only `ls`. Elevation
/// applies only to the **prefixed** form, where the variable is scoped to this
/// command; a *bare* assignment (`PATH=/tmp` with no command) instead poisons
/// later stages, which [`pipeline`](super::pipeline) handles as a binding
/// disturbance.
pub fn normalize_invocation(words: &[String]) -> Command {
    normalize_as_written(words, &[])
}

/// [`normalize_invocation`], told which words the shell may split into several
/// (`splits[i]`, read from the source as written; absent means none do).
///
/// A wrapper's own word that splits moves where the inner command starts:
/// `timeout $X cat f` with `X='1 sh -c id'` runs `sh`, not `cat`. So the peel
/// stops there, as it does at a wrapper flag it does not know, and the wrapper
/// is the command — which asks (Reviewer #3). A split AFTER the inner program
/// (`timeout 10 cat $F`) changes its arguments, not which program runs.
///
/// Sets [`Command::words_may_split`] — except for the shell's own leading
/// assignments: bash neither splits, globs nor brace-expands `X=$(ls)`, and
/// counting it made a bare assignment ask for a flag it could not hide. After
/// a wrapper, `X=$Y` is an ordinary argument (`env X=$Y cmd`), and does split.
pub(crate) fn normalize_as_written(words: &[String], splits: &[bool]) -> Command {
    let mut env = BTreeMap::new();
    let mut wrappers: Vec<String> = Vec::new();
    let mut wrapper_args: Vec<String> = Vec::new();
    let mut wrapper_argc: Vec<usize> = Vec::new();
    let mut rest = words;
    let assigned_by_the_shell = words.len() - split_assignments(words).1.len();

    // Peel assignments and wrappers until we reach the real command. `env FOO=1
    // sudo nice rm -rf /` is an `rm`. Bounded so a pathological `sudo sudo …`
    // cannot spin; hitting the bound just stops peeling, leaving the wrapper as
    // the command (Executor), which is the conservative direction.
    for _ in 0..MAX_PEEL {
        let (found, remainder) = split_assignments(rest);
        let at = words.len() - remainder.len();
        // After a wrapper its `X=$Y` is an argument, which splits: `env X=$Y
        // cat f` with `Y='1 rm -rf /'` runs `rm`.
        if splits
            .get(assigned_by_the_shell.max(words.len() - rest.len())..at)
            .is_some_and(|own| own.contains(&true))
        {
            break;
        }
        env.extend(found);
        rest = remainder;
        let Some((program, argv)) = rest.split_first() else {
            break;
        };
        let Some(model) = super::program::wrapper(program) else {
            break;
        };
        let Some(inner) = inner_index(program, model.kind, argv) else {
            break; // no inner command found — treat the wrapper itself as the command
        };
        if splits
            .get(at + 1..at + 1 + inner)
            .is_some_and(|own| own.contains(&true))
        {
            break;
        }
        wrappers.push(program.clone());
        // The wrapper's own words run too — keep them (see `Command::wrapper_args`).
        wrapper_args.extend_from_slice(&argv[..inner]);
        wrapper_argc.push(inner);
        rest = &argv[inner..];
    }

    let mut command = match rest.split_first() {
        Some((program, argv)) => normalize(program, argv),
        // A bare assignment (`FOO=bar`) runs nothing itself.
        None => Command {
            program: String::new(),
            words: Vec::new(),
            env: BTreeMap::new(),
            flags: Vec::new(),
            operands: Vec::new(),
            unknown: Vec::new(),
            unmodeled_flags: Vec::new(),
            wrappers: Vec::new(),
            wrapper_args: Vec::new(),
            wrapper_argc: Vec::new(),
            effect: Effect::ReadOnly,
            words_may_split: false,
            expansion: Vec::new(),
        },
    };

    // Hazardous variables elevate the command they are scoped to. Applied after
    // unwrapping, so `LD_PRELOAD=x sudo ls` elevates the real command.
    if !command.program.is_empty() {
        for name in env.keys() {
            // `join`, not a hand-written rank comparison: this was the third
            // home of the security-relevant "raise to the worst", in the one
            // path `join`'s own doc says it exists for (Reviewer D).
            if let Some(hazard) = super::program::env_hazard(name) {
                command.effect = command.effect.join(hazard.effect);
            }
        }
    }
    command.env = env;
    command.wrappers = wrappers;
    command.wrapper_args = wrapper_args;
    command.wrapper_argc = wrapper_argc;
    command.words_may_split = splits.iter().skip(assigned_by_the_shell).any(|&s| s);
    command
}

/// How many wrapper/assignment layers we will peel before giving up.
const MAX_PEEL: usize = 8;

/// Where the inner command begins inside a wrapper's `argv` — after the
/// wrapper's own flags, plus one positional for `timeout 5 cmd` / `flock file
/// cmd`. `None` if nothing is left, **or if a flag is one the wrapper's model
/// does not know** — in which case the wrapper is treated as the command
/// itself (an executor, which asks).
///
/// **Fail closed.** This was a second flag grammar that read an unknown
/// dashed word as a switch — the fail-OPEN reading, because a flag it did
/// not know could take the next word as its value: `env -u ls rm -R
/// ~/project` peeled to an `ls`, Allow and safe unattended, under the
/// default policy (SECURITY-2026-09-30 R2-1). An unknown flag can also change
/// WHERE or WHAT runs (`env -C /`, `env -S 'rm -rf /'`), which no peel can
/// see past. It reads flags through the one grammar ([`find_arg`],
/// [`split_bundle`]); a wrapper's everyday flags are modelled so that rules
/// about the command inside still see it.
fn inner_index(program: &str, kind: super::program::Wrap, argv: &[String]) -> Option<usize> {
    let model = lookup(program);
    let mut i = 0;
    while i < argv.len() {
        let token = &argv[i];
        if token == "--" {
            i += 1;
            break;
        }
        if !is_flag_word(token) {
            break;
        }
        let model = model?;
        let (word, inline) = match token.split_once('=') {
            Some((w, _)) => (w, true),
            None => (token.as_str(), false),
        };
        if let Some(arg) = find_arg(model, word) {
            i += 1;
            if arg.takes_value && !inline {
                i += 1;
            }
            continue;
        }
        if model.bundles_shorts
            && let Some((_, took_next)) = split_bundle(model, token, argv.get(i + 1))
        {
            i += 1 + usize::from(took_next);
            continue;
        }
        return None;
    }
    if kind == super::program::Wrap::AfterFlagsAndOneOperand {
        i += 1;
    }
    (i < argv.len()).then_some(i)
}

/// Split leading `NAME=VALUE` assignments off an invocation, returning them and
/// the remaining words (whose first entry, if any, is the program).
fn split_assignments(words: &[String]) -> (BTreeMap<String, String>, &[String]) {
    let mut env = BTreeMap::new();
    let mut i = 0;
    while i < words.len() {
        match assignment(&words[i]) {
            Some((name, value)) => {
                env.insert(name, value);
                i += 1;
            }
            None => break,
        }
    }
    (env, &words[i..])
}

/// `NAME=VALUE` where NAME is a shell identifier. Rejects `--flag=v` (leading
/// dash) and `=v`, so only genuine assignments are stripped.
fn assignment(word: &str) -> Option<(String, String)> {
    let (name, value) = word.split_once('=')?;
    // `NAME+=value` (append) is an assignment too; the name is NAME. Without
    // this, `PATH+=/tmp` looked like an ordinary program and did not disturb
    // name resolution for later stages. (Reviewer E.)
    let name = name.strip_suffix('+').unwrap_or(name);
    let mut chars = name.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some((name.to_string(), value.to_string()))
}

/// Classify `argv` (the words *after* the program name) against `program`'s
/// model into a [`Command`]. An unmodeled program yields
/// [`Effect::Unknown`](super::Effect::Unknown) — we do not guess its argument
/// grammar or its safety.
pub fn normalize(program: &str, argv: &[String]) -> Command {
    // Both returns below, so a command built either way carries them.
    let words: Vec<String> = std::iter::once(program.to_string())
        .chain(argv.iter().cloned())
        .collect();
    let Some(model) = lookup(program) else {
        // No model: keep the operands as-is (best effort) and mark the whole
        // command Unknown so callers treat it conservatively.
        return Command {
            program: program.to_string(),
            words,
            env: BTreeMap::new(),
            flags: Vec::new(),
            operands: argv.to_vec(),
            unknown: Vec::new(),
            // Its words stay operands (containment grades them), and are read
            // again as flags for the rules that name one.
            unmodeled_flags: unmodeled_words(argv),
            wrappers: Vec::new(),
            wrapper_args: Vec::new(),
            wrapper_argc: Vec::new(),
            effect: Effect::Unknown,
            words_may_split: false,
            expansion: Vec::new(),
        };
    };

    let mut flags: Vec<Flag> = Vec::new();
    let mut operands: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut unmodeled_flags: Vec<Flag> = Vec::new();
    // The danger of known letters inside bundles that were refused whole.
    let mut hidden = Effect::ReadOnly;

    let mut i = 0;
    let mut flags_ended = false;
    while i < argv.len() {
        let token = &argv[i];
        i += 1;

        if flags_ended {
            operands.push(token.clone());
            continue;
        }
        // Where the program's grammar says so: `find`'s `--` ends nothing
        // the gate reads (`Program::dashdash_ends_flags`).
        if token == "--" {
            flags_ended = model.dashdash_ends_flags();
            continue;
        }
        if !is_flag_word(token) {
            operands.push(token.clone());
            continue;
        }

        // A dashed token. Long (`--x`) and single-dash forms both allow an
        // inline `=value`.
        let (word, inline) = match token.split_once('=') {
            Some((w, v)) => (w, Some(v.to_string())),
            None => (token.as_str(), None),
        };

        if let Some(arg) = find_arg(model, word) {
            let value = if arg.takes_value {
                inline.or_else(|| {
                    // Consume the next token as the value, if there is one.
                    (i < argv.len()).then(|| {
                        let v = argv[i].clone();
                        i += 1;
                        v
                    })
                })
            } else {
                None
            };
            // Pushed, never inserted: a repeated flag is two flags (see
            // `Command::flags` for what a map here let through).
            flags.push(Flag {
                name: arg.canonical.to_string(),
                value,
            });
            continue;
        }

        // `head -20`: a dashed number is a flag with its value, for the
        // programs where it is one (`program::dash_number`).
        if let Some(canonical) = super::program::dash_number(program)
            && let Some(digits) = token.strip_prefix('-')
            && !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
        {
            flags.push(Flag {
                name: canonical.to_string(),
                value: Some(digits.to_string()),
            });
            continue;
        }

        // A bundle of shorts on a bundling program (`-rf`, `-cvf a.tar`,
        // `-f/etc/x`), read as getopt reads it. Otherwise keep the token as
        // unknown.
        if model.bundles_shorts
            && let Some((bundled, took_next)) = split_bundle(model, token, argv.get(i))
        {
            flags.extend(bundled);
            if took_next {
                i += 1;
            }
            continue;
        }

        // For `echo` and `test`, a dashed word that is none of their flags is
        // DATA (`program::dashes_are_data`) — an operand like any other.
        if super::program::dashes_are_data(program) {
            operands.push(token.clone());
            continue;
        }

        // Recognized as a flag by shape, but not by this program: keep it —
        // never drop a token, never silently ignore it. `i` is already past
        // it, so `argv.get(i)` is the word after: the value it may carry,
        // which stays an operand as well.
        unknown.push(token.clone());
        let unmodeled = Flag::unmodeled(token, argv.get(i));
        // **A bundle refused for one unknown letter still carries its known
        // letters' danger**: `sort -rXo out` writes `out` whatever `X` is.
        // Without this, an elevating short on a bundling program could not be
        // modelled at all (a test forbade it), so `sort -o` could not be.
        if model.bundles_shorts {
            hidden = unmodeled
                .bundled_shorts()
                .filter_map(|short| find_arg(model, &short)?.elevates)
                .fold(hidden, Effect::join);
        }
        unmodeled_flags.push(unmodeled);
    }

    let mut effect = elevated_effect(model, &flags).join(hidden);
    // `uniq IN OUT` writes `OUT`: an operand position that is a write.
    if super::program::writes_operand(program).is_some_and(|at| operands.len() > at) {
        effect = effect.join(Effect::Mutating);
    }
    Command {
        program: program.to_string(),
        words,
        env: BTreeMap::new(),
        wrappers: Vec::new(),
        wrapper_args: Vec::new(),
        wrapper_argc: Vec::new(),
        flags,
        operands,
        unknown,
        unmodeled_flags,
        effect,
        words_may_split: false,
        expansion: Vec::new(),
    }
}

/// Find the [`Arg`](super::program::Arg) whose spellings include `word` — or,
/// for a long option, the one arg it is an unambiguous PREFIX of.
///
/// GNU `getopt_long` accepts any unique prefix: `rm --no /` is
/// `rm --no-preserve-root /`, and `--rec` is `--recursive`. Read only by its
/// full spelling, `allow rm:*` ran `rm -rf --no /` while the shipped
/// `deny rm:--no-preserve-root` refused the full one (SECURITY-2026-09-30
/// #4). An ambiguous prefix is an error to getopt, so it is no flag here
/// either — it stays unknown, which asks. A program without long options
/// (BSD `rm`) refuses the prefix outright, so reading it as the flag only
/// errs toward a prompt.
fn find_arg<'a>(model: &'a Program, word: &str) -> Option<&'a super::program::Arg> {
    if let Some(exact) = model.args.iter().find(|a| a.spellings.contains(&word)) {
        return Some(exact);
    }
    if !word.starts_with("--") || word.len() <= 2 {
        return None;
    }
    let mut prefixed = model.args.iter().filter(|a| {
        a.spellings
            .iter()
            .any(|s| s.starts_with("--") && s.starts_with(word))
    });
    let only = prefixed.next()?;
    prefixed.next().is_none().then_some(only)
}

/// **An unmodeled program's words, read as flags** — every dashed word up to
/// the `--` that ends them, each with the value it may carry.
///
/// A `--` right after a flag with no inline value may BE that value — a flag
/// that takes one takes the next word, `--` included (`curl -o -- -k u`
/// writes to a file named `--` and still reads `-k`). Stopping there anyway
/// hid `-k` from `deny curl -k` (SECURITY-2026-09-30 #6), so such a `--`
/// ends nothing. Reading on past a `--` that did end the flags only makes an
/// operand look like a flag: more than the program sees, which a deny may
/// over-read and an allow cannot be widened by (it covers any operand).
fn unmodeled_words(argv: &[String]) -> Vec<Flag> {
    let mut out = Vec::new();
    for (i, word) in argv.iter().enumerate() {
        if word == "--" {
            let a_value = i
                .checked_sub(1)
                .map(|p| &argv[p])
                .is_some_and(|prev| is_flag_word(prev) && prev != "--" && !prev.contains('='));
            if !a_value {
                break;
            }
            continue;
        }
        if is_flag_word(word) {
            out.push(Flag::unmodeled(word, argv.get(i + 1)));
        }
    }
    out
}

/// **A bundle of shorts, read as getopt reads one** — its flags, and whether
/// the last one took `next` as its value.
///
/// Letters run until one takes a value, which takes the REST of the word —
/// or, when the rest is empty, the next word: `-cvf a.tar` is `-c -v -f
/// a.tar`, `-f/etc/x` is `-f` with `/etc/x`, `-rf` is `-r -f`. `None` when a
/// letter before that is not this program's: the token stays whole and
/// unknown, a caution signal — which letters follow an unknown one, and
/// which of them takes a value, is a guess.
///
/// It refused ANY bundle holding a value-taking letter ("ambiguous — refuse
/// to guess"), which a dashed getopt bundle is not. So `tar -cf /etc/x` kept
/// `-cf` in `unknown` and `/etc/x` among the operands, where no rule about
/// `--file` could see it, and `-f.env.local` hid a credential path from the
/// gate inside one token (SECURITY-2026-09-30 #3).
fn split_bundle(model: &Program, token: &str, next: Option<&String>) -> Option<(Vec<Flag>, bool)> {
    let body = token.strip_prefix('-').filter(|b| !b.starts_with('-'))?;
    let mut flags = Vec::new();
    for (at, letter) in body.char_indices() {
        let arg = find_arg(model, &format!("-{letter}"))?;
        if arg.takes_value {
            let rest = &body[at + letter.len_utf8()..];
            let (value, took_next) = match rest {
                "" => (next.cloned(), next.is_some()),
                attached => (Some(attached.to_string()), false),
            };
            flags.push(Flag {
                name: arg.canonical.to_string(),
                value,
            });
            return Some((flags, took_next));
        }
        flags.push(Flag {
            name: arg.canonical.to_string(),
            value: None,
        });
    }
    // Every letter a switch — and at least one, or this is no bundle.
    (!flags.is_empty()).then_some((flags, false))
}

/// The command's effect: its program's base, raised by any present flag that
/// elevates. (Unrecognized flags do not elevate here — they are surfaced via
/// [`Command::has_unknown`](super::Command::has_unknown) for the caller to treat
/// cautiously — value-based danger inspection is a later refinement.)
fn elevated_effect(model: &Program, flags: &[Flag]) -> Effect {
    let mut effect = model.base;
    for flag in flags {
        if let Some(arg) = model.args.iter().find(|a| a.canonical == flag.name)
            && let Some(e) = arg.elevates
        {
            effect = effect.join(e);
        }
    }
    effect
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze(s: &str) -> Command {
        super::super::analyze(s).expect("analyzable")
    }

    /// THE property this whole module exists to guarantee: a flag set is a set,
    /// not a spelling. Every way of writing `rm -r -f` yields the same flags —
    /// so a rule can never be bypassed by reordering, bundling, or aliasing.
    #[test]
    fn rm_rf_is_the_same_command_however_you_spell_it() {
        let want = ["force", "recursive"]; // a BTreeSet → sorted names
        for spelling in [
            "rm -rf x",
            "rm -fr x",
            "rm -r -f x",
            "rm -f -r x",
            "rm --recursive --force x",
            "rm --force --recursive x",
            "rm -r --force x",
        ] {
            let cmd = analyze(spelling);
            let got: Vec<&str> = cmd.flag_names().into_iter().collect();
            assert_eq!(got, want, "flag set differs for {spelling:?}");
            assert_eq!(cmd.operands, vec!["x"], "operands differ for {spelling:?}");
        }
    }

    #[test]
    fn value_flags_normalize_across_forms() {
        for spelling in [
            "tar --file=a.tar -c",
            "tar --file a.tar -c",
            "tar -f a.tar -c",
        ] {
            let cmd = analyze(spelling);
            let files: Vec<&str> = cmd.flag_values("file").collect();
            assert_eq!(files, ["a.tar"], "for {spelling:?}");
            assert!(cmd.has_flag("create"));
        }
    }

    /// A flag given twice is two flags. A map kept the last value only, so
    /// every check reading values was shown a different command from the one
    /// that runs (SECURITY-2026-09-29): the credential gate read `a.tar`,
    /// never the key file. Every spelling, every position, both kept.
    #[test]
    fn a_repeated_flag_keeps_every_value_in_order() {
        for spelling in [
            "tar -x --file=~/.ssh/id_rsa --file=a.tar",
            "tar -x -f ~/.ssh/id_rsa --file a.tar",
            "tar --file ~/.ssh/id_rsa -x -f a.tar",
        ] {
            let cmd = analyze(spelling);
            let files: Vec<&str> = cmd.flag_values("file").collect();
            assert_eq!(files, ["~/.ssh/id_rsa", "a.tar"], "for {spelling:?}");
        }
        let find = analyze("find . -name '*.pem' -name '*'");
        let names: Vec<&str> = find.flag_values("name").collect();
        assert_eq!(names, ["*.pem", "*"]);
        // The set view stays a set: two `-name`s are one name in it.
        assert_eq!(find.flag_names().into_iter().collect::<Vec<_>>(), ["name"]);
        // Every value reaches the unresolved check too, not only the first
        // (SECURITY-2026-09-30 #11: reading one passed every test).
        assert!(analyze("tar -x --file=a.tar --file=$X").has_unresolved_operand());
        assert!(!analyze("tar -x --file=a.tar --file=b.tar").has_unresolved_operand());
    }

    /// **A bundle is read as getopt reads it**: letters until one takes a
    /// value, which takes the rest of the word or the next word. An unknown
    /// letter before that keeps the token whole (SECURITY-2026-09-30 #3).
    #[test]
    fn a_bundle_splits_as_getopt_splits_it() {
        let flags = |line: &str| -> (Vec<(String, Option<String>)>, Vec<String>) {
            let cmd = analyze(line);
            let flags = cmd.flags.into_iter().map(|f| (f.name, f.value)).collect();
            (flags, cmd.operands)
        };
        let f = |name: &str, value: Option<&str>| (name.to_string(), value.map(str::to_string));
        assert_eq!(
            flags("tar -cvf a.tar x"),
            (
                vec![
                    f("create", None),
                    f("verbose", None),
                    f("file", Some("a.tar"))
                ],
                vec!["x".into()]
            )
        );
        assert_eq!(
            flags("tar -f/etc/x -c"),
            (vec![f("file", Some("/etc/x")), f("create", None)], vec![])
        );
        assert_eq!(
            flags("tar -xzf a.tar").0,
            [
                f("extract", None),
                f("gzip", None),
                f("file", Some("a.tar"))
            ]
        );
        assert_eq!(
            flags("tar -czfq x").0.last(),
            Some(&f("file", Some("q"))),
            "the rest"
        );
        let unknown = analyze("tar -xqf a.tar");
        assert_eq!(unknown.unknown, ["-xqf"], "an unknown letter first: whole");
        assert!(unknown.operands.contains(&"a.tar".to_string()));
    }

    /// `echo`'s own letters bundle into options, as bash's builtin reads them;
    /// any other dashed word is text — for `test`, every one is.
    #[test]
    fn echo_and_test_keep_dashed_data_as_operands() {
        let echo = analyze("echo -ne x -nx ---");
        assert!(echo.has_flag("no-newline") && echo.has_flag("escapes"));
        assert_eq!(echo.operands, ["x", "-nx", "---"]);
        assert!(echo.unknown.is_empty());
        let test = analyze("test -d src -a -f x");
        assert_eq!(test.operands, ["-d", "src", "-a", "-f", "x"]);
        assert!(test.unknown.is_empty());
    }

    /// A unique prefix of a long option is that option; an ambiguous one is
    /// an error to getopt and stays unknown here.
    #[test]
    fn a_unique_long_prefix_is_the_long_option() {
        assert!(analyze("rm --rec x").has_flag("recursive"));
        assert!(analyze("rm --no /").has_flag("no-preserve-root"));
        let ambiguous = analyze("tar --c");
        assert_eq!(
            ambiguous.unknown,
            ["--c"],
            "--create or --checkpoint-action"
        );
        assert!(
            analyze("rm -- --rec")
                .operands
                .contains(&"--rec".to_string())
        );
    }

    /// A word may become a flag only if its expansion can START with a dash:
    /// nothing literal before its first expansion, or a dash.
    #[test]
    fn only_a_word_that_can_start_with_a_dash_may_become_a_flag() {
        use super::super::could_become_a_flag as may;
        for word in [
            "$X",
            "${X}",
            "*",
            "?x",
            "[-]rf",
            "{a,-rf}",
            "-{true,delete}",
            "-$X",
            "$()",
            "$D/f",
        ] {
            assert!(may(word), "{word}");
        }
        for word in [
            "src/*.rs", "./$X", "a$X", "x{a,b}", "~", "~/x", "-rf", "plain", "",
        ] {
            assert!(!may(word), "{word}");
        }
        // A flag's value is not an operand, so it is not asked about.
        assert_eq!(analyze("find . -name $X").hidden_flag(), None);
        assert_eq!(analyze("find . $X -name y").hidden_flag(), Some("$X"));
    }

    /// The words no model named, read as flags with the value each may carry
    /// — for an unmodeled program (all its words are operands) and for a
    /// modeled program's unknown tokens alike. Nothing moves: the operands are
    /// what they were, so containment grades the same words.
    #[test]
    fn unmodeled_flags_keep_their_spelling_and_the_word_after() {
        let curl = analyze("curl -k --url=https://a -o out -- -z");
        let got: Vec<(&str, Option<&str>)> = curl
            .unmodeled_flags
            .iter()
            .map(|f| (f.name.as_str(), f.value.as_deref()))
            .collect();
        assert_eq!(
            got,
            [
                ("-k", Some("--url=https://a")), // a candidate: -k might take one
                ("--url", Some("https://a")),
                ("-o", Some("out")),
            ],
            "and nothing after `--`"
        );
        assert_eq!(curl.operands.len(), 6, "every word is still an operand");

        let ls = analyze("ls --frob=1 --wibble x");
        let names: Vec<&str> = ls.unmodeled_flags.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["--frob", "--wibble"]);
        assert_eq!(ls.unknown, ["--frob=1", "--wibble"], "unknown is unchanged");
        assert_eq!(ls.operands, ["x"]);
    }

    #[test]
    fn unknown_program_is_unknown_not_assumed_safe() {
        let cmd = analyze("frobnicate --wibble target");
        assert_eq!(cmd.effect, Effect::Unknown);
        assert!(!cmd.is_modeled());
    }

    #[test]
    fn unknown_flags_are_kept_never_dropped() {
        let cmd = analyze("rm --frobnicate x");
        assert_eq!(cmd.unknown, vec!["--frobnicate"]);
        assert!(cmd.has_unknown());
        assert_eq!(cmd.operands, vec!["x"]);
    }

    #[test]
    fn a_bundle_with_an_unknown_letter_stays_whole() {
        // `-rz`: r is known, z is not → refuse to split, keep the token.
        let cmd = analyze("rm -rz x");
        assert!(cmd.flags.is_empty());
        assert_eq!(cmd.unknown, vec!["-rz"]);
    }

    #[test]
    fn find_is_read_only_until_a_flag_elevates_it() {
        assert_eq!(
            analyze("find . -name *.rs -type f").effect,
            Effect::ReadOnly
        );
        assert_eq!(analyze("find . -delete").effect, Effect::Destructive);
        // Real `find -exec` escapes the terminator (`\;`); the backslash makes
        // the `;` a literal operand rather than a command separator.
        assert_eq!(analyze("find . -exec rm {} \\;").effect, Effect::Executor);
    }

    #[test]
    fn executors_are_flagged_by_identity() {
        // The operand is a program in another language; THIS layer does not
        // parse it, and knows `awk` runs arbitrary code. The gate does read
        // awk's script (`permission::model::lang::awk`), and decides by its
        // acts — so `awk '{print}'` reads there while it is an executor here;
        // this pins the shell layer's own reading (review stop 12, T8: it
        // read as the gate's). (The script is single-quoted, as it must be —
        // its `()` would otherwise be shell structure.)
        assert_eq!(analyze("awk 'BEGIN{system(id)}'").effect, Effect::Executor);
        assert_eq!(analyze("sh -c whatever").effect, Effect::Executor);
    }

    #[test]
    fn single_quotes_make_operators_literal() {
        // The `()` are inside single quotes → literal text, not shell structure.
        let toks = tokenize_simple("awk 'BEGIN { system() }'").unwrap();
        assert_eq!(toks, vec!["awk", "BEGIN { system() }"]);
    }

    #[test]
    fn compounds_and_substitutions_fail_closed() {
        assert_eq!(
            tokenize_simple("a && b"),
            Err(Unanalyzable::ShellStructure { found: "&&" })
        );
        assert_eq!(
            tokenize_simple("a | b"),
            Err(Unanalyzable::ShellStructure { found: "|" })
        );
        assert_eq!(
            tokenize_simple("a ; b"),
            Err(Unanalyzable::ShellStructure { found: ";" })
        );
        assert_eq!(
            tokenize_simple("echo > f"),
            Err(Unanalyzable::ShellStructure { found: ">" })
        );
        assert_eq!(
            tokenize_simple("echo $(whoami)"),
            Err(Unanalyzable::Substitution)
        );
        assert_eq!(
            tokenize_simple("echo `whoami`"),
            Err(Unanalyzable::Substitution)
        );
        assert_eq!(
            tokenize_simple("echo 'unterminated"),
            Err(Unanalyzable::UnterminatedQuote)
        );
        assert_eq!(tokenize_simple("   "), Err(Unanalyzable::Empty));
    }

    /// The bypass this exists to close: an env-var prefix must not hide the
    /// program. If `FOO=bar rm` parsed as a program named `FOO=bar`, every deny
    /// rule about `rm` would silently stop applying — a rule you can evade by
    /// prepending a harmless variable is not a rule.
    #[test]
    fn an_env_prefix_does_not_hide_the_program() {
        let cmd = analyze("FOO=bar rm -rf /");
        assert_eq!(cmd.program(), "rm");
        assert!(cmd.has_flag("recursive") && cmd.has_flag("force"));
        assert_eq!(cmd.effect, Effect::Destructive);
        assert_eq!(cmd.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(cmd.operands, vec!["/"]);
    }

    #[test]
    fn several_assignments_are_all_parsed() {
        let cmd = analyze("FOO=1 BAR=2 ls -l");
        assert_eq!(cmd.program(), "ls");
        assert_eq!(cmd.env.len(), 2);
        assert_eq!(cmd.effect, Effect::ReadOnly, "harmless vars change nothing");
    }

    /// A hazardous variable turns *any* program into an executor — the env-var
    /// equivalent of `find -exec`. A read-only `ls` under LD_PRELOAD is not one.
    #[test]
    fn hazardous_env_vars_elevate_the_effect() {
        assert_eq!(analyze("LD_PRELOAD=/evil.so ls").effect, Effect::Executor);
        assert_eq!(
            analyze("DYLD_INSERT_LIBRARIES=/x ls").effect,
            Effect::Executor
        );
        assert_eq!(
            analyze("NODE_OPTIONS=--require=/x ls").effect,
            Effect::Executor
        );
        // PATH voids the *identity*: this may not be the ls we think it is.
        assert_eq!(analyze("PATH=/tmp ls").effect, Effect::Unknown);
        // On an *unmodeled* program the command is already Unknown, which
        // outranks Executor. Elevation must never "downgrade" us to a
        // classification we cannot actually justify.
        assert_eq!(
            analyze("NODE_OPTIONS=--require=/x node app.js").effect,
            Effect::Unknown
        );
    }

    #[test]
    fn env_hazards_explain_themselves() {
        let cmd = analyze("LD_PRELOAD=/evil.so ls");
        let hazards = cmd.env_hazards();
        assert_eq!(hazards.len(), 1);
        assert_eq!(hazards[0].name, "LD_PRELOAD");
        assert!(
            !hazards[0].why.is_empty(),
            "the prompt must be able to say why"
        );
    }

    /// `--flag=value` is not an assignment; only a leading shell identifier is.
    #[test]
    fn flag_values_are_not_mistaken_for_assignments() {
        let cmd = analyze("tar --file=a.tar -c");
        assert_eq!(cmd.program(), "tar");
        assert!(cmd.env.is_empty());
        assert_eq!(cmd.flag_values("file").collect::<Vec<_>>(), ["a.tar"]);
    }

    /// We resolve neither variables nor globs, so a path-scoped rule must know
    /// the operand isn't a literal path — `$DIR` could be `/`.
    #[test]
    fn unresolved_operands_are_reported() {
        assert!(analyze("rm -rf $DIR").has_unresolved_operand());
        assert!(analyze("rm -rf ~/x").has_unresolved_operand());
        assert!(analyze("rm -rf *.log").has_unresolved_operand());
        // Brace expansion happens before we ever see the words, and `/{,tmp}`
        // expands to two paths — one of which is `/` (BashPitfalls #33).
        assert!(analyze("rm -rf /{,tmp}").has_unresolved_operand());
        assert!(!analyze("rm -rf /tmp/build").has_unresolved_operand());
    }

    #[test]
    fn double_dash_ends_flag_parsing() {
        let cmd = analyze("rm -f -- -r");
        // After `--`, `-r` is an operand (a file literally named "-r"), not the
        // recursive flag.
        assert!(cmd.has_flag("force"));
        assert!(!cmd.has_flag("recursive"));
        assert_eq!(cmd.operands, vec!["-r"]);
    }
}

#[cfg(test)]
mod wrapper_args_tests {
    use super::*;

    fn analyze_line(s: &str) -> Command {
        let words: Vec<String> = s.split_whitespace().map(str::to_string).collect();
        normalize_invocation(&words)
    }

    /// A wrapper's own argv EXECUTES, so it must survive the peel. Asserts the
    /// CONTENT, not just that the vector is non-empty: the failure this guards
    /// against is the field existing and staying empty, which compiles, passes
    /// any is-it-there check, and silently scans a subset of what runs.
    #[test]
    fn wrapper_own_argv_survives_the_peel() {
        let c = analyze_line("timeout 5 cat f");
        assert_eq!(c.program(), "cat");
        assert_eq!(c.wrappers, vec!["timeout".to_string()]);
        assert_eq!(
            c.wrapper_args,
            vec!["5".to_string()],
            "the wrapper's own operand must be kept, not discarded"
        );
    }

    /// The reason it matters: a credential path written where the wrapper's own
    /// argument goes is a path that runs.
    #[test]
    fn a_path_in_wrapper_argv_is_still_visible() {
        let c = analyze_line("flock /home/me/.ssh/id_rsa cat f");
        assert!(
            c.wrapper_args.iter().any(|a| a.contains(".ssh")),
            "a sensitive path in the wrapper's argv must remain scannable, got {:?}",
            c.wrapper_args
        );
    }

    /// `xargs` makes the visible operands incomplete, so a path-scoped rule
    /// would match vacuously against an empty list.
    ///
    /// **Pins a known limit, deliberately.** `argv_from_stream` searches the
    /// peeled *wrappers*, so a bare `xargs` — which peels nothing, because
    /// there is no inner command to find — is NOT detected. The last assertion
    /// records that rather than hiding it; widening it to the program name is a
    /// docketed proposal, not a silent change made during a recovery.
    #[test]
    fn argv_from_stream_finds_the_wrapper() {
        assert_eq!(analyze_line("xargs cat").argv_from_stream(), Some("xargs"));
        assert_eq!(analyze_line("cat f").argv_from_stream(), None);
        assert_eq!(analyze_line("timeout 5 cat f").argv_from_stream(), None);
        assert_eq!(
            analyze_line("xargs").argv_from_stream(),
            None,
            "known limit: a bare `xargs` peels nothing, so the name never \
             reaches the wrapper list"
        );
    }

    /// And the caveat signal must reflect it — read by the pane's colour
    /// (`stage_tone`), not by the decision, which reads acts (this said
    /// "every permission decision reads": review stop 12, T8).
    #[test]
    fn stream_fed_argv_makes_operands_unresolved() {
        assert!(analyze_line("xargs cat").has_unresolved_operand());
        assert!(!analyze_line("cat f").has_unresolved_operand());
    }
}
