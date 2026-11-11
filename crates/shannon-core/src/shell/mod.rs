//! Shell-command analysis — the input layer the harness (permissions, danger
//! highlighting, pitfall hooks) will consume.
//!
//! # Why this exists
//!
//! Claude Code, sudo, and every glob-over-a-string permission system share one
//! root flaw: they match rules against **command text**. Text has many spellings
//! for one operation — `rm -rf`, `rm -fr`, `rm -r -f`, `rm --recursive --force`
//! are the same command, and a text rule catches one of them. A *deny* rule you
//! can reorder around is worse than no rule, because it manufactures confidence.
//! sudo concluded, after forty years, "never restrict with wildcards." That
//! verdict is conditional on matching text. We match **structure** instead.
//!
//! [`analyze`] turns command text into a [`Command`]: a program plus an
//! order-free **flag set** (every spelling collapsed to one canonical name),
//! operands, unrecognized tokens (kept, never dropped), and an [`Effect`] —
//! what the command can *do*. A rule then asks structural questions
//! ("does the flag set contain `delete`?", "is the effect `Executor`?") that
//! hold regardless of how the command was written.
//!
//! # Two principles baked in
//!
//! - **No universal argv grammar, so per-program knowledge with a conservative
//!   default.** Unix CLIs disagree (`rm -rf` bundles, `find -delete` does not,
//!   `ps aux` drops the dash, `dd if=…` is key=value). Each program we model
//!   carries its own rules ([`program`]); an unmodeled program is
//!   [`Effect::Unknown`] — never assumed safe.
//! - **Fail closed.** [`analyze`] refuses (`Err`) anything it will not fully
//!   interpret — compound commands, pipes, redirections, `$(…)`. Refusing to
//!   analyze is safe; analyzing a fragment and calling it safe is not.
//!
//! # Coverage: every layer between "what was typed" and "what runs"
//!
//! Each of these is a place the text and the reality can diverge, and each is
//! therefore an attack surface. The list is derived from the **shell's own
//! processing order**, not from the bugs we happened to hit — a checklist we can
//! audit against, rather than a memory of past whack-a-mole. Anything not
//! **handled** must be **refused** or **flagged**; nothing may be silently
//! assumed benign.
//!
//! | shell step | status |
//! |---|---|
//! | tokenizing & quote removal | handled ([`lex()`](lex::lex)) |
//! | alias expansion | tracked — [`Bindings`](pipeline::Bindings) |
//! | brace expansion `{a,b}` | flagged unresolved (not expanded) |
//! | tilde expansion `~` | flagged unresolved |
//! | parameter expansion `$VAR` | flagged unresolved |
//! | command substitution `` $() `` / backticks | analyzed — the inner command is its own stage; its value is unresolved |
//! | arithmetic expansion `$(( ))` | **refused** |
//! | process substitution `<( )` `>( )` | analyzed — the inner command is its own stage |
//! | word splitting (`IFS`) | `IFS` is an [`EnvHazard`](program::EnvHazard) |
//! | filename globbing `* ? [` | flagged unresolved |
//! | redirection `>` `>>` `<`, `N>`, `&>` | handled (an *effect*, target kept; exactly `/dev/null` writes nothing) |
//! | fd duplication `2>&1`, `>&-` | handled — names no file |
//! | an expansion that becomes a FLAG (`$X`, `*`, `-{a,b}`) | flagged — [`Command::hidden_flag`] |
//! | heredoc `<<`, `<<-` | handled — a quoted (or literal) body is data, read only for secret handles; a body that expands is **refused** |
//! | herestring `<<<` | **refused** |
//! | command separators `; & && \|\| \|` and **newline** | handled |
//! | backgrounding `&` | **refused** |
//! | subshell / grouping `( )` `{ }` | **refused** |
//! | control flow `for while until if` | decomposed — the commands inside are analyzed |
//! | control flow `case select function coproc` | **refused** ([`Unanalyzable::ControlFlow`]) |
//! | env assignment prefixes `FOO=bar cmd` | handled (parsed, and they elevate) |
//! | wrapper programs `sudo nice timeout env` | handled (peeled to the real command) |
//! | name resolution via `PATH` / `hash` / functions | tracked — [`Bindings`](pipeline::Bindings) |
//! | `CDPATH` redirecting `cd` (BashPitfalls #48) | tracked — later `cd`s become [`Cwd::Unknown`](pipeline::Cwd) |
//! | `xargs` taking operands from stdin (#56) | flagged — operands not complete |
//! | pipeline stages running in **subshells** | tracked — a `cd` there doesn't propagate |
//! | ANSI-C quoting `$'\\x2f'`, locale `$"..."` | **refused** — decodes to something else |
//! | bash 5.3 `${ cmd; }` / `${\| cmd; }` | **refused** — runs a command without a subshell |
//! | extended globs `@() !() +()`, `**` | **refused** / flagged |
//!
//! **Known gaps, stated rather than hidden** (each fails closed today, but none
//! is *resolved*): the inner command of `sh -c '…'` / `su -c '…'` is a quoted
//! string we do not re-lex (those stay [`Effect::Executor`]); we do not resolve
//! `$PATH` to the binary that would actually run; `cd` through a symlink is
//! lexical only; heredoc *contents* are never analyzed; danger carried in a flag
//! *value* (`tar --checkpoint-action=exec=…`) is caught by the flag's presence,
//! not its value; and shell functions defined in the user's rc files are
//! invisible to us — which is why the execution model must not source them (see
//! `docs/Milestones/Milestone 4.md`).
//!
//! ## The one gap worth naming separately: expansion can inject *flags*
//!
//! An operand that still expands can become a **flag**, not just a path. A
//! file literally named `-delete` turns `find . *` into a deletion; an
//! unquoted `$VAR` holding `-rf` turns `rm $VAR` into a recursive force. So
//! when operands are unresolved, the flag set we can see is **not final**, and
//! neither is the [`Effect`] derived from it.
//!
//! **Handled since 2026-09-30** ([`Command::hidden_flag`], SECURITY-2026-09-30
//! #1). This section used to defer it: raising the effect would fire on the
//! ordinary `find . -name '*.rs'`, and telling `'*.rs'` from `*.rs` needs quote
//! tracking. Both halves were narrower than they looked. `'*.rs'` there is
//! `-name`'s VALUE, and a value cannot become a flag; and a word can become
//! one only if its expansion can START with a dash — its literal prefix is
//! empty or dash-led — which is exact without knowing about quotes (a quoted
//! `"$X"` still becomes `-delete`). Meanwhile the deferral was not the "fail
//! closed" the paragraph above claims: the default policy ran
//! `X=-delete; find . $X`, and `allow find:*` ran `find . -{true,delete}`
//! past the shipped carvings.
//!
//! Quote tracking would still make [`Command::has_unresolved_operand`]
//! precise, which over-reports a quoted glob as unresolved (safe, but noisy).
//!
//! # What this layer intentionally does *not* do (the seams)
//!
//! Kept out so we don't build a roadblock before the design is settled:
//!
//! - **Layer A — compound decomposition.** Splitting `a && b | c` into simple
//!   commands, with an abstract state model so `cd / && rm -rf *` and
//!   `cd /tmp && rm -rf *` are told apart, needs a real shell parser. Until it
//!   lands we fail closed on compounds. `analyze` is deliberately the
//!   *simple-command* entry point that decomposition will call.
//! - **The rule grammar / permission engine.** How rules are written, matched,
//!   laddered, and learned is a separate, still-open design (see
//!   `docs/Milestones/Milestone 4.md`). This module only produces the structure
//!   rules match *against*.
//! - **Value-based danger** (`tar --checkpoint-action=exec=…`), wrapper
//!   stripping (`nice`/`timeout`/`sudo`), and `$PATH` resolution — noted where
//!   relevant, handled later.

pub mod lex;
mod normalize;
pub(crate) mod param;
pub mod pipeline;
pub mod program;
pub mod provenance;

pub use lex::{Lexeme, Operator, Spanned, lex};
pub use normalize::{normalize, normalize_invocation, tokenize_simple};
pub use pipeline::{Cwd, Pipeline, Redirect, RedirectKind, Stage, parse_line, subst_inner_span};
pub use program::{Arg, Program, Wrap, Wrapper};
// `provenance::analyze` is NOT re-exported flat — `analyze` already names the
// simple-command entry point below. Callers use `provenance::analyze`; the
// output contract `Flow` is re-exported for convenience.
pub use provenance::Flow;

use std::collections::BTreeMap;

/// What a command can do — the axis that decides how broad a permission rung is
/// safe to offer, because *effect class, not program name*, is what matters
/// (`find` is harmless until `-exec`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Only observes state (`ls`, `cat`, `find` searching). Safe to allow broadly.
    ReadOnly,
    /// Changes state within scope (`mkdir`, `cp`, `tar -x`).
    Mutating,
    /// Irreversibly removes (`rm`, `shred`, `find -delete`).
    Destructive,
    /// Can execute arbitrary other programs — a shell, a language runtime, or an
    /// escape hatch (`sh`, `awk`, `xargs`, `find -exec`). **Never** a broad rung.
    Executor,
    /// We have no model for this program, so we cannot classify it. Treat
    /// conservatively; never assume [`ReadOnly`](Effect::ReadOnly).
    Unknown,
}

impl Effect {
    /// Severity ordering for "raise the effect to the most dangerous present."
    /// [`Unknown`](Effect::Unknown) sorts highest so that if it is ever mixed in,
    /// it dominates (fail closed).
    pub(crate) fn rank(self) -> u8 {
        match self {
            Effect::ReadOnly => 0,
            Effect::Mutating => 1,
            Effect::Destructive => 2,
            Effect::Executor => 3,
            Effect::Unknown => u8::MAX,
        }
    }

    /// The more dangerous of two effects — the lattice join. This is a genuine
    /// join, not an arbitrary order: the rank is a capability chain (an
    /// `Executor` can also destroy, so it dominates `Destructive`) with
    /// `Unknown` on top. One helper for every "raise to the worst" site, so the
    /// security-relevant comparison can't drift between copies. (Reviewers C, D.)
    pub(crate) fn join(self, other: Effect) -> Effect {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }
}

/// Does this token still carry a shell expansion or glob — a variable (`$`),
/// tilde, or filename pattern (`* ? [ {`) — so it is **not** a literal we can
/// resolve? Shared by [`Command::has_unresolved_operand`] and the `cd`
/// directory tracker so the two can't disagree about what counts as resolvable
/// (they did: the `cd` side was missing `[` and `{`). (Reviewers D, E.)
pub(crate) fn expands(token: &str) -> bool {
    token.contains('$')
        || token.starts_with('~')
        || token.contains(['*', '?', '['])
        || token
            .match_indices('{')
            .any(|(at, _)| opens_brace(token, at))
}

/// **Does the `{` at byte `at` of `text` open a brace expansion?** Bash
/// expands `{a,b}` and `{1..9}` and leaves any other brace as itself: `{}`,
/// `{x}`, and the placeholders `find -exec` and `fd -x` fill in (`{}`, `{.}`,
/// `{/}`, `{//}`). Every `{` read as an expansion, `find . -exec rm {} \;`
/// was a hidden flag no allow could cover — harmless while `-exec` asked
/// anyway, live once `allow find *` answers it (Steven, 10-06).
///
/// Read wide, the safe side: a `,` or `..` ANYWHERE after it in the word
/// opens one, quoted or not. Bash closes a group at its MATCHING `}` — by
/// depth, past a quoted or escaped one — and a reading that stopped at the
/// first `}` called `{x{}y,-delete}`, `{x'}'y,-delete}` and `{x\}y,-delete}`
/// literal, so `find . {x{}y,-delete}` ran unasked under the shipped rules
/// (SECURITY-2026-10-06, Reviewer B). Every group bash expands holds a `,` or
/// `..` before its end, which is before the word's: a superset, never a miss.
/// `text` is ONE word — every caller's. The one reading of a brace for
/// [`expands`], [`expands_as_written`], [`could_become_a_flag`] and
/// [`splits_unquoted`]; the glob readers that take every brace as wild
/// (`pattern::glob_body`, the gate's literal prefix and suffix) only ask more.
pub(crate) fn opens_brace(text: &str, at: usize) -> bool {
    let rest = &text[at + 1..];
    rest.contains(',') || rest.contains("..")
}

/// **Does a word, as WRITTEN, still carry an expansion the shell performs?**
/// Read from the source with its quotes: a `$` outside single quotes (a
/// double-quoted `"$X"` still expands), a `* ? [ {` outside any quotes. A
/// backslash makes the next character literal. `'*.rs'` does not — the
/// shell hands the program the text `*.rs` — while [`expands`], which reads
/// the quote-removed text, cannot tell the two apart. A tilde is not asked
/// here; see [`Command::expands`].
///
/// **And a backtick, as a `$` is** — `` `cmd` `` is `$(cmd)` written the
/// old way, and expands outside single quotes. It was missing, so a word
/// holding one read as quoted text: `` find . `echo -delete` `` was no
/// hidden flag, and `` rm build/`echo x` `` was covered by `allow rm
/// build/*` as the literal file `build/$()` (SECURITY-2026-10-05, the
/// backtick; Reviewer A).
pub(crate) fn expands_as_written(source: &str) -> bool {
    let (mut single, mut double) = (false, false);
    let mut chars = source.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' if !single => {
                chars.next();
            }
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '$' | '`' if !single => return true,
            '*' | '?' | '[' if !single && !double => return true,
            '{' if !single && !double && opens_brace(source, at) => return true,
            _ => {}
        }
    }
    false
}

/// **`text` as one shell word**, single-quoted, so the gate reads a path as
/// the file it names whatever it holds — a space, a quote, a `$`. How a tool
/// call that is not a command is said to the gate (`claude_code::reading`,
/// `files::reading`): one quoting, so the two read a path alike.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// **Could this word expand into a flag?** Only if its expansion can START
/// with `-`: it holds an expansion (`$`, a glob, a brace) and what comes
/// before the first one is empty or dash-led. `src/*.rs`, `./$X` and `a$X`
/// always expand to something beginning with their literal prefix, so they
/// cannot; `$X`, `*`, `$(…)`, `[-]rf`, `-{a,b}` — and `$DIR/f`, which is
/// `-x/f` when `DIR` is `-x` — can. That last one is conservative (most
/// programs reject `-x/f` rather than run it), deliberately: "most" is not a
/// guarantee. A tilde cannot — it becomes a path.
///
/// Exact without knowing whether the expansion was quoted, which the lexer
/// does not record: a quoted `$X` still becomes a flag, and a quoted glob with
/// an empty prefix (`'*'`) is rare where a flag could go.
pub fn could_become_a_flag(word: &str) -> bool {
    // `~+` and `~-` are `$PWD` and `$OLDPWD`, which can be `-delete` like any
    // variable (SECURITY-2026-09-30 R2-3); every other tilde is a home.
    word.starts_with("~+")
        || word.starts_with("~-")
        || tilde_by_brace(word)
        || word
            .char_indices()
            .find(|&(at, c)| {
                matches!(c, '$' | '*' | '?' | '[') || (c == '{' && opens_brace(word, at))
            })
            .is_some_and(|(at, _)| at == 0 || word.starts_with('-'))
}

/// **Can a brace make this word's tilde `~-`, `~+` or `~N`?** Bash expands
/// braces BEFORE tildes, so `~{x,-}` is `~x ~-` — `$OLDPWD` — and `~{x,}-`
/// the same: a brace that opens anywhere in the tilde's prefix (up to the
/// first `/`) can write any of them, and each is a variable the line may
/// have set to `-delete`. `OLDPWD=-delete; find . -name ~{x,-}` ran as
/// `find . -name ~x -delete` under the shipped rules (SECURITY-2026-10-06,
/// Reviewer E2). Read wide: any such brace — an extra ask, never a miss.
pub(crate) fn tilde_by_brace(word: &str) -> bool {
    word.starts_with('~')
        && word
            .char_indices()
            .take_while(|&(_, c)| c != '/')
            .any(|(at, c)| c == '{' && opens_brace(word, at))
}

/// **Can this word, AS WRITTEN, expand into several — one of them a flag?**
///
/// Why it matters: a word that splits can put a FLAG after itself, even where
/// it stands as a flag's value — `find . -name {x,-delete}` is `find . -name
/// x -delete` to bash, and `-name $X` with `X='x -o -delete'` likewise
/// (SECURITY-2026-09-30 R2-3: the first fix held that a value can never
/// become a flag, which is true only of a value that stays one word).
///
/// - An UNQUOTED `$` word-splits, and what it splits into comes from the
///   variable, whatever is written before it: `/tmp/$X` with `X='x -r'` is
///   `/tmp/x -r`. So any unquoted `$` can.
/// - `"$@"`, `"${@}"` and `"${a[@]}"` split inside double quotes too.
/// - An unquoted glob or brace list expands to words that all BEGIN with the
///   literal text before it: `src/*.rs` cannot, `*.rs` and `-{a,b}` can —
///   and so can `~{x,-}`, whose `~-` becomes `$OLDPWD` after the braces.
/// - Nothing quoted splits: `-name '*.rs'`, `-name "$N"` stay quiet.
///
/// Read from the source slice, the one place quoting survives.
pub(crate) fn splits_unquoted(source: &str) -> bool {
    let mut chars = source.char_indices().peekable();
    let (mut single, mut double) = (false, false);
    // The quote-removed text so far, before any expansion: what every word
    // a glob or brace list yields begins with.
    let mut literal = String::new();
    let prefix_can_be_a_flag = |literal: &str| literal.is_empty() || literal.starts_with('-');
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' if !single => {
                if let Some((_, escaped)) = chars.next() {
                    literal.push(escaped);
                }
            }
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            // `"$@"`, and any `@` in a braced expansion: `"${@}"`, `"${@:2}"`,
            // `"${a[@]}"`, `"${!pre@}"` all split inside quotes. (`"${x@Q}"`
            // does not, and is caught anyway: an extra Ask, never a missed flag.)
            '$' if double => {
                let splat = match chars.peek().map(|&(_, c)| c) {
                    Some('@') => true,
                    Some('{') => chars
                        .clone()
                        .map(|(_, c)| c)
                        .take_while(|&c| c != '}')
                        .any(|c| c == '@'),
                    _ => false,
                };
                if splat {
                    return true;
                }
                literal.push(c);
            }
            c if single || double => literal.push(c),
            // A backtick substitution word-splits as `$(…)` does
            // (`expands_as_written`).
            '$' | '`' => return true,
            '*' | '?' | '[' if prefix_can_be_a_flag(&literal) => return true,
            // A brace in a tilde's prefix can make it `~-` — `$OLDPWD` —
            // since bash expands braces first (`tilde_by_brace`).
            '{' => {
                let tilde = literal.starts_with('~') && !literal.contains('/');
                if opens_brace(source, at) && (prefix_can_be_a_flag(&literal) || tilde) {
                    return true;
                }
                literal.push(c);
            }
            c => literal.push(c),
        }
    }
    false
}

/// A single simple command, normalized: the identity a rule matches against.
///
/// # Soundness contract (read before building the rule engine)
///
/// [`effect`](Command::effect) is a **lower bound**, not the whole safety
/// verdict. A modeled program carrying an *unrecognized* flag stays at its base
/// effect (a novel dangerous `find` action reads as `ReadOnly`), and an operand
/// that still expands isn't a known path. So a permission decision must treat
/// the effect as sound **only in conjunction with** the caveat signals:
/// [`has_unknown`](Command::has_unknown), [`has_unresolved_operand`](Command::has_unresolved_operand),
/// and — at the line level — [`Pipeline::bindings_trusted`] and
/// [`Pipeline::cwd_fully_known`]. Any of them true means "I could not fully
/// account for this": do not grant a broad rung on the effect alone. Keying on
/// `effect` by itself is the most likely way a real consumer reintroduces a
/// bypass. (Reviewer E.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// The program as written (basename or path). Use [`program`](Command::program).
    /// Empty for a bare assignment (`FOO=bar` with no command).
    pub program: String,
    /// **The words the program is run with, as the shell passes them** — the
    /// program as written, then each argument with its quotes removed, the
    /// wrappers and leading assignments already peeled: `sudo FOO=1 "rm" -rf
    /// x` is `["rm", "-rf", "x"]`. What a `re:` rule reads, joined by single
    /// spaces (`permission/rule.rs`). The one field that keeps what was
    /// written, in order: `flags` holds canonical names, `operands` drops the
    /// flags. Empty for a bare assignment.
    pub words: Vec<String>,
    /// Leading `NAME=VALUE` assignments (`FOO=bar cmd`). Parsed rather than
    /// treated as the program name — otherwise `FOO=bar rm -rf /` would look
    /// like a program called `FOO=bar` and a rule about `rm` would not match it.
    pub env: BTreeMap<String, String>,
    /// Every flag given, **each occurrence kept, in the order written**, by
    /// canonical name with its value if it takes one. `-rf`, `-fr`, and
    /// `--recursive --force` all name the same flags; ask
    /// [`has_flag`](Command::has_flag), [`flag_values`](Command::flag_values) or
    /// [`flag_names`](Command::flag_names), which are order-free.
    ///
    /// **It was a map, and a map keeps one value per key.** A repeated flag
    /// kept its LAST value, so every check that reads values saw one where the
    /// command has several: `tar -x --file=~/.ssh/id_rsa --file=a.tar` ran
    /// without asking under `allow tar:*` while the one-`--file` spelling asked,
    /// and `find . -name '*.pem' -name '*'` listed key files the plain
    /// `-name '*.pem'` stopped for (SECURITY-2026-09-29). Order-free is a
    /// property of the questions; the record keeps what was written.
    pub flags: Vec<Flag>,
    /// Positional arguments, in order (paths, patterns, subcommands).
    pub operands: Vec<String>,
    /// Dashed tokens the program's model didn't recognize — **kept, never
    /// dropped**. Their presence is a caution signal (see [`has_unknown`](Command::has_unknown)).
    pub unknown: Vec<String>,
    /// Every dashed word **no model named**, by its spelling (`--url`, `-k`),
    /// each with the value it may carry: its inline `=value`, else the word
    /// after it. Where a rule's literal flag (`deny curl -k`,
    /// `deny kubectl --context=prod*`) is looked for.
    ///
    /// For a modeled program these are the [`unknown`](Command::unknown)
    /// tokens; for an unmodeled one, whose words are all operands (it has no
    /// grammar to say otherwise), the dashed operands before any `--`. It adds
    /// no word the other fields lack — it re-reads them with the ADJACENCY
    /// they drop, because a value written `--url x` is otherwise one operand
    /// among many.
    ///
    /// The value is a *candidate*: without a model we cannot know that a flag
    /// takes one, so a boolean flag's candidate is really an operand. That
    /// only matters to a rule that constrains a value of a flag which takes
    /// none.
    ///
    /// **Before this, a rule's literal flag was looked for in `unknown` alone,
    /// by exact token** — so every rule naming a flag of an unmodeled program
    /// was inert (`allow curl:*` + `deny curl -k` ran `curl -k`), a literal
    /// flag was missed as `--frob=1`, and a literal flag's value was never
    /// read at all. Found by the probe for SECURITY-2026-09-29.
    pub unmodeled_flags: Vec<Flag>,
    /// Wrapper programs peeled off to reach this command, outermost first
    /// (`sudo nice rm …` -> ["sudo", "nice"]). Recorded rather than
    /// discarded: privilege (`sudo`) is a real axis a rule must be able to
    /// see, separate from what KIND of action this is.
    pub wrappers: Vec<String>,
    /// The wrapper's OWN argv — its flags, their values, and `timeout`/`flock`'s
    /// positional — kept rather than discarded.
    ///
    /// `normalize_invocation` peels `timeout 5 cat f` to a `cat`, and used to
    /// drop the `5` and everything beside it. The shell does not drop them, so
    /// a token there is a token that executes while landing in no field any
    /// check reads: `timeout {{tok.1a2b}} cat f` spent a lent credential
    /// unattended, because `expand_at_exec` scans the raw line and the gate
    /// scans fields (Reviewer E).
    ///
    /// Scanned by the permission engine’s `stage_paths`, which makes
    /// `stage_tokens`, `has_unresolved_operand` and `path_indeterminate`
    /// correct for free — one accessor, so they cannot diverge again.
    pub wrapper_args: Vec<String>,
    /// How many of [`wrapper_args`](Self::wrapper_args) each wrapper owns,
    /// parallel to [`wrappers`](Self::wrappers): `sudo -u pg nice ls` is
    /// `[2, 0]`. A rule on a privilege wrapper names its flags (`allow sudo
    /// -u=pg psql *`), so they are read per wrapper, not as one list.
    pub wrapper_argc: Vec<usize>,
    /// What this command can do, program base raised by any elevating flag.
    pub effect: Effect,
    /// **Some word, as written, may expand to several** (`splits_unquoted`)
    /// — so a flag can appear that no word we see is, even from a flag's
    /// value. Set by the pipeline, which alone still has the words' quoting
    /// (the normalizer sees them with quotes removed); read by
    /// [`flags_are_final`](Command::flags_are_final).
    pub words_may_split: bool,
    /// **Each word of the stage — wrappers and assignments too — and whether
    /// the shell still expands it** (`expands_as_written`). Set by the
    /// pipeline, which alone has the words' quoting; empty elsewhere, which
    /// [`expands`](Command::expands) reads as "every expansion-shaped word is
    /// live", the answer before quoting was kept.
    pub expansion: Vec<(String, bool)>,
}

/// One flag as written: which flag it is, and the value it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flag {
    /// In [`Command::flags`], the program model's name for it, whatever the
    /// spelling (`-r`, `-R` and `--recursive` are all `recursive`). In
    /// [`Command::unmodeled_flags`], the spelling itself (`--url`), since no
    /// model names it.
    pub name: String,
    /// Its value, if it takes one and one was given.
    pub value: Option<String>,
}

impl Flag {
    /// A dashed word as a flag: `--url=x` is `--url` with the value `x`;
    /// otherwise the value is the `next` word, whatever it is — a flag that
    /// takes a value takes the next word even when it is dashed (getopt's
    /// rule, and the one the modeled path follows when it consumes a value).
    fn unmodeled(word: &str, next: Option<&String>) -> Flag {
        let (name, inline) = match word.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (word, None),
        };
        Flag {
            name: name.to_string(),
            value: inline.or_else(|| next.cloned()),
        }
    }

    /// The short flags this word would bundle, if its program bundles
    /// (`-sk` → `-s`, `-k`; `-4k` → `-4`, `-k`; `-ko/tmp/o` → `-k`, `-o`,
    /// the rest being `-o`'s attached value); nothing for a long `--flag`,
    /// whose second dash is not an option character. A
    /// question only of an [unmodeled](Command::unmodeled_flags) word: a
    /// modeled program's clusters were already split by its model — or
    /// refused, which is how one lands here.
    ///
    /// Read the way getopt reads one: option characters from the start, until
    /// the first that cannot be one — without a model we cannot know which
    /// letter takes the rest as its value, so every character up to there
    /// counts. It required every character to be a LETTER, and `curl -4k`,
    /// `-ko/tmp/o` hid their `-k` (SECURITY-2026-09-30 #5).
    pub fn bundled_shorts(&self) -> impl Iterator<Item = String> + '_ {
        self.name
            .strip_prefix('-')
            .unwrap_or("")
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '#')
            .map(|c| format!("-{c}"))
    }
}

/// Does this word read as a flag: dashed, and not the lone `-` (which names
/// stdin/stdout by convention and is an operand)? `--` is a flag word too —
/// the one that ends flags.
pub(crate) fn is_flag_word(word: &str) -> bool {
    word.starts_with('-') && word != "-"
}

impl Command {
    /// The program name.
    pub fn program(&self) -> &str {
        &self.program
    }
    /// Is this flag present, by any spelling?
    pub fn has_flag(&self, canonical: &str) -> bool {
        self.flags.iter().any(|f| f.name == canonical)
    }
    /// **Every** value given to this flag, in order (`--file a --file b` →
    /// `a`, `b`).
    ///
    /// Deliberately no accessor for "the" value: which occurrence a program
    /// honours is the program's business (`find` ANDs every `-name`; others
    /// keep the last, or use each in turn), and a check that picks one reads a
    /// command other than the one that runs. Say which quantifier you mean
    /// over all of them.
    pub fn flag_values<'a>(&'a self, canonical: &str) -> impl Iterator<Item = &'a str> {
        self.flags
            .iter()
            .filter(move |f| f.name == canonical)
            .filter_map(|f| f.value.as_deref())
    }
    /// The canonical names present, once each — the spelling- and order-free
    /// set a rule's flag predicate is about.
    pub fn flag_names(&self) -> std::collections::BTreeSet<&str> {
        self.flags.iter().map(|f| f.name.as_str()).collect()
    }
    /// Do we have a model for this program? `false` ⇒ [`Effect::Unknown`], treat
    /// conservatively.
    pub fn is_modeled(&self) -> bool {
        self.effect != Effect::Unknown
    }
    /// Did the command carry dashed tokens we couldn't classify? A rule should
    /// hesitate to grant a broad rung when it can't account for every argument.
    pub fn has_unknown(&self) -> bool {
        !self.unknown.is_empty()
    }

    /// The hazardous environment variables this command sets, with the reason
    /// each is hazardous — for the approval prompt and for `hints`.
    pub fn env_hazards(&self) -> Vec<&'static program::EnvHazard> {
        self.env
            .keys()
            .filter_map(|k| program::env_hazard(k))
            .collect()
    }

    /// Does any argument fail to name a literal path — because it contains a
    /// variable/expansion (`$X`, `~`) or a filename glob (`*`, `?`, `[`)?
    ///
    /// **Path-scoped rules must fail closed when this is true.** `rm -rf $DIR`
    /// is a fine `rm` by structure, but `$DIR` could be `/`; and `rm -rf "$V/"`
    /// with an empty `V` is the classic pitfall that deletes the root. We resolve
    /// neither the shell's variables nor its globbing, so we must not pretend an
    /// operand names a known file.
    pub fn has_unresolved_operand(&self) -> bool {
        self.argv_from_stream().is_some()
            || self.operands.iter().any(|s| expands(s))
            || self
                .flags
                .iter()
                .filter_map(|f| f.value.as_deref())
                .any(expands)
    }

    /// **A word the shell has yet to expand that could become a FLAG** — the
    /// first one, if any.
    ///
    /// `$X` holding `-delete`, `*` over a file named `-delete`, `-{true,delete}`
    /// (brace expansion: `find . -true -delete`). What such a word becomes is
    /// decided when the line runs, so the flags we can see are not the ones
    /// that run — and neither is the effect derived from them, nor the verdict
    /// of any rule about flags. Under the default policy `X=-delete; find . $X`
    /// ran without a prompt (SECURITY-2026-09-30 #1).
    ///
    /// Only an operand or an unrecognized dashed word, and only one whose
    /// expansion can START with a dash ([`could_become_a_flag`]). A flag's
    /// VALUE cannot: `-name $X` is a name whatever `$X` holds. Quoting does not
    /// save one: `find "$X"` passes `-delete` as a single word, which is a
    /// flag.
    pub fn hidden_flag(&self) -> Option<&str> {
        // And only one the shell still expands ([`expands`](Self::expands)):
        // `fd '*.toml'` hands fd the text, which no file named `-x` can
        // become — it read as a live glob, and every quoted pattern of a
        // program some flag makes worse asked (10-05, modelling `fd`).
        // `"$X"` still expands, and still counts.
        self.operands
            .iter()
            .chain(self.unknown.iter())
            .map(String::as_str)
            .find(|word| could_become_a_flag(word) && self.expands(word))
    }

    /// **Are the flags we can see all the flags that will run?** Not when a
    /// word may expand into one ([`hidden_flag`](Self::hidden_flag)), and not
    /// when the argument list comes from a stream
    /// ([`argv_from_stream`](Self::argv_from_stream)): `ls | xargs find .`
    /// runs `find` with whatever names `ls` prints, and a file named
    /// `-delete` is one (SECURITY-2026-09-30 #7). The one question the gate,
    /// the rule check and the warning colour all ask.
    pub fn flags_are_final(&self) -> bool {
        self.hidden_flag().is_none() && self.argv_from_stream().is_none() && !self.words_may_split
    }

    /// **Does the shell still expand `token`** — one of this command's words,
    /// or a piece of one (a flag's value)? `shell::expands` by its text, unless a
    /// quoted word holds it and no live one does ([`expansion`]): `rg -g
    /// '!**/tests*' x` hands rg the text, so an allow that read it covers it,
    /// while in `rg '*.rs' *` the bare `*` is still the shell's glob.
    ///
    /// The quoting was lost at the lexer, so every quoted `*` read as a glob:
    /// no allow could cover `rg … --glob '!**/tests*'` — most of a day's asked
    /// `rg` calls (2026-10-05). A leading `~` keeps that text answer: a
    /// quoted one is literal to the shell, but Claude Code's file tools
    /// expand it, and Shannon reads their calls as quoted shell lines.
    ///
    /// [`expansion`]: Command::expansion
    pub fn expands(&self, token: &str) -> bool {
        if !expands(token) || token.starts_with('~') {
            return expands(token);
        }
        let held = |live: bool| {
            self.expansion
                .iter()
                .any(|(word, is_live)| *is_live == live && word.contains(token))
        };
        !held(false) || held(true)
    }

    /// The wrapper that builds this command's **real argument list from a
    /// stream**, if any — so the operands we can see are not the ones that
    /// will be acted on (BashPitfalls #56).
    ///
    /// A named accessor rather than a term inside one predicate, because two
    /// predicates answer "can we say what this touches" and only one of them
    /// had this term. `find . -exec cat {} \;` is carved out by the shipped
    /// rules (it asks);
    /// `find . -type f | xargs cat` is the same capability, and it was
    /// `safe_unattended` — a child would read every file under the cwd,
    /// unattended, including the ones the credential seed list names
    /// (Reviewer E, round 3, CRITICAL).
    ///
    /// Treating the visible operands as complete lets a path-scoped rule match
    /// **vacuously against an empty list**, which is the fail-open direction.
    pub fn argv_from_stream(&self) -> Option<&str> {
        self.wrappers
            .iter()
            .find(|w| matches!(w.rsplit('/').next().unwrap_or(w), "xargs"))
            .map(String::as_str)
    }
}

/// Why a command string could not be analyzed. Every variant is a *deliberate*
/// refusal to guess; the reason is user-facing (it both explains the prompt and
/// can teach — "this contains `$(…)`, which runs another command").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unanalyzable {
    /// Empty or whitespace-only input.
    Empty,
    /// Shell structure the pipeline does not decompose (a subshell `( … )`,
    /// a background `&`, a here-string) or cannot make sense of (an empty
    /// segment, unbalanced control flow). The whole line is refused. (`|`,
    /// `&&`, `;` and redirects reach this only from [`analyze`], the one
    /// simple command; `parse_line` takes them apart.)
    ShellStructure {
        /// The operator we stopped on, for a clear message.
        found: &'static str,
    },
    /// Command substitution — `$(…)` or backticks — which embeds another command
    /// whose text isn't known statically. Refused when it cannot be captured
    /// whole (unbalanced, or inside double quotes).
    Substitution,
    /// Arithmetic expansion `$((…))`. No command in it — but bash evaluates a
    /// variable named in it AS an expression, and an array subscript there runs
    /// `$(…)`: `X='a[$(rm -rf ~)]'; echo $((X))` runs the `rm`. It was reported
    /// as "command substitution", which it is not.
    Arithmetic,
    /// A quote was opened and never closed.
    UnterminatedQuote,
    /// A control-flow construct we do not decompose **yet** (`case`/`esac`,
    /// `select`, `function`, `coproc`, `{ }` grouping, `[[ ]]`).
    ///
    /// Note this is *not* "control flow is unknowable" — `for`, `while`,
    /// `until`, and `if` are decomposed into the commands they run (see
    /// [`peel_control_flow`](pipeline)). It fires only where we have not yet
    /// enumerated which positions hold **commands** versus **words**, and
    /// guessing that wrong would silently skip a command.
    ControlFlow {
        /// The keyword we stopped on.
        keyword: &'static str,
    },
}

impl Unanalyzable {
    /// A short, user-facing explanation.
    pub fn reason(&self) -> String {
        match self {
            Unanalyzable::Empty => "empty command".into(),
            // It said "— analyzed per simple command", from when a compound
            // line went through the analyzer one command at a time; now this
            // refuses the WHOLE line, under a pane headed "not analyzed", and
            // the two contradicted each other (Steven's demo, 2026-10-03).
            Unanalyzable::ShellStructure { found } => format!(
                "contains a shell construct Shannon does not take apart yet ({found}), so \
                 nothing in it was analyzed"
            ),
            Unanalyzable::Substitution => {
                "contains command substitution ($(…) or backticks) — runs another command".into()
            }
            Unanalyzable::Arithmetic => "contains arithmetic $((…)) — bash evaluates the \
                                         variables in it as code, so it is not read past"
                .into(),
            Unanalyzable::UnterminatedQuote => "has an unterminated quote".into(),
            Unanalyzable::ControlFlow { keyword } => {
                format!("contains a shell construct we don't decompose yet ({keyword})")
            }
        }
    }
}

impl std::fmt::Display for Unanalyzable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason())
    }
}

/// Analyze one **simple** command string into a normalized [`Command`], or
/// return why it couldn't be ([`Unanalyzable`]). This is the module's entry
/// point; compound decomposition (Layer A) will call it per simple command.
pub fn analyze(input: &str) -> Result<Command, Unanalyzable> {
    let tokens = tokenize_simple(input)?;
    let mut command = normalize_invocation(&tokens);
    // One command, so the whole line is its words as written.
    command.words_may_split = splits_unquoted(input);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// **A refused line says it was not analyzed** — the pane is headed so,
    /// and the reason said "analyzed per simple command" beneath it
    /// (Steven's demo, 2026-10-03: a subshell).
    #[test]
    fn a_refused_line_says_nothing_in_it_was_analyzed() {
        let why = parse_line("ls && (echo a) || echo b").expect_err("a subshell is refused");
        let said = why.reason();
        assert!(said.contains("(subshell)"), "{said}");
        assert!(said.contains("nothing in it was analyzed"), "{said}");
    }

    /// **A brace expands only as bash expands it** (`opens_brace`), in every
    /// predicate that reads one: a `,` or `..` in the group. `{}` and `fd`'s
    /// placeholders are text — and a group read wide (quoted, nested,
    /// unclosed) still expands, the safe side.
    #[test]
    fn a_brace_expands_only_with_a_list_or_a_range() {
        for text in ["{}", "{.}", "{/}", "{//}", "{/.}", "{x}", "a{}b"] {
            assert!(!expands(text), "{text}");
            assert!(!expands_as_written(text), "{text}");
            assert!(!could_become_a_flag(text), "{text}");
            assert!(!splits_unquoted(text), "{text}");
        }
        // The last four close past an inner, quoted or escaped `}`, as bash's
        // matching brace does (SECURITY-2026-10-06).
        for text in [
            "{x,-delete}",
            "{1..9}",
            "{a,{b,c}}",
            "{a,b",
            "{x{}y,-delete}",
            "{{},-delete}",
            "{x'}'y,-delete}",
            "{x\\}y,-delete}",
        ] {
            assert!(expands(text), "{text}");
            assert!(expands_as_written(text), "{text}");
            assert!(could_become_a_flag(text), "{text}");
            assert!(splits_unquoted(text), "{text}");
        }
        // `{}x {}y` to bash. Read wide, the `{}` opens too, so it COULD be a
        // flag here — an extra ask, the price of a reading that is sound on
        // quote-removed text, where `{x'}'y,W}` is `{x}y,W}` and a reading
        // by depth would close it early.
        assert!(expands("{}{x,y}") && expands_as_written("{}{x,y}"));
        assert!(could_become_a_flag("{}{x,y}"), "the safe side");
        assert!(!could_become_a_flag("src/{a,b}"), "a literal prefix");
        assert!(!expands_as_written("'{a,b}'"), "quoted");
    }

    /// The six permutations of three indices — used to prove order-invariance
    /// without depending on a shuffle strategy.
    const PERMS: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];

    proptest! {
        /// The core guarantee, as a property: for the three boolean flags of
        /// `rm`, *every* ordering and *every* short/long spelling produces the
        /// identical canonical flag set. This is the invariant Claude Code's
        /// exact and prefix rule forms cannot express, and that defeats the
        /// `rm -rf` vs `rm -fr` bypass.
        #[test]
        fn flag_set_is_invariant_under_order_and_spelling(
            perm_idx in 0usize..6,
            long_r in any::<bool>(),
            long_f in any::<bool>(),
            long_i in any::<bool>(),
        ) {
            let spellings = [
                if long_r { "--recursive" } else { "-r" },
                if long_f { "--force" } else { "-f" },
                if long_i { "--interactive" } else { "-i" },
            ];
            let perm = PERMS[perm_idx];
            let ordered: Vec<&str> = perm.iter().map(|&idx| spellings[idx]).collect();
            let line = format!("rm {} target", ordered.join(" "));

            let cmd = analyze(&line).expect("analyzable");
            let keys: Vec<&str> = cmd.flag_names().into_iter().collect();
            prop_assert_eq!(keys, vec!["force", "interactive", "recursive"]);
            prop_assert_eq!(cmd.operands, vec!["target"]);
            prop_assert_eq!(cmd.effect, Effect::Destructive);
        }
    }

    #[test]
    fn reason_is_human_readable() {
        assert!(Unanalyzable::Substitution.reason().contains("$(…)"));
        assert!(
            Unanalyzable::ShellStructure { found: "|" }
                .to_string()
                .contains('|')
        );
    }

    /// **What the shell still expands is read with the quotes** — a `$` in
    /// double quotes expands, nothing in single quotes does, and no glob
    /// character in either.
    #[test]
    fn an_expansion_as_written_respects_its_quotes() {
        for live in ["*.rs", "\"$X\"", "a*", "{a,b}", "x$Y", "'a'*", "\"a\"[b]"] {
            assert!(expands_as_written(live), "{live}");
        }
        for text in [
            "'*.rs'", "\"*.rs\"", "'$X'", r"\*.rs", "a'*'b", "'{a,b}'", "plain", "'!**/x'",
        ] {
            assert!(!expands_as_written(text), "{text}");
        }
    }

    /// **A word is literal only where every word holding it was quoted**: in
    /// `rg '*.rs' *` the quoted `*.rs` is text and the bare `*` the shell's.
    #[test]
    fn a_command_knows_which_of_its_words_still_expand() {
        let stage = |line: &str| {
            pipeline::parse_line(line)
                .expect("analyzable")
                .stages
                .remove(0)
                .command
        };
        let cmd = stage("rg '*.rs' *");
        assert!(!cmd.expands("*.rs"));
        assert!(cmd.expands("*"), "a bare glob is live, beside a quoted one");
        let cmd = stage("rg -g '!**/tests*' x");
        assert!(!cmd.expands("!**/tests*"));
        assert!(
            stage("cat \"$X\"").expands("$X"),
            "double quotes expand `$`"
        );
        assert!(!stage("cat '$X'").expands("$X"));
        assert!(
            stage("cat '~/x'").expands("~/x"),
            "a tilde keeps its old answer"
        );
        // Without the source (`analyze` of a bare string), every glob-shaped
        // word reads as live, as before.
        assert!(analyze("rg '*.rs'").expect("ok").expands("*.rs"));
    }
}
