//! Running an approved shell command.
//!
//! This is the *activity* half of a turn: the side-effecting step the model
//! asked for, a human approved, and we then actually perform. Kept in one small
//! module with a channel-shaped interface so the policy above it (who decides)
//! and the transport below it (how it runs) stay separable — a different
//! executor (a sandbox, a remote runner) is a swap here, not a rewrite.
//!
//! # What it borrows from durable-workflow engines
//!
//! A turn behaves like a workflow and a tool call like an activity, so the
//! well-trodden discipline applies:
//!
//! - **Start-to-close timeout.** An activity that never returns must not hang
//!   the workflow. [`ExecConfig::timeout`] bounds every run, and the child is
//!   *killed* — not merely abandoned — when it expires.
//! - **Bounded output.** A command that prints forever is the same failure in a
//!   different costume, so output is capped as it is read rather than after.
//! - **The result belongs in the history.** The `tool_result` is sealed into the
//!   turn, so what ran and what it produced is recoverable later.
//!
//! # Where it deliberately differs
//!
//! **Retries are opt-in and default to off.** Workflow engines retry activities
//! by default, because activities are usually idempotent code the author wrote.
//! A shell command proposed by a model is neither: re-running `rm -rf` after a
//! timeout could delete twice, and a timeout tells us the command did not
//! *finish*, not that it did nothing. So the default is zero — a retry is a
//! human pressing the key again — while [`ExecConfig::max_retries`] lets someone
//! who knows their commands are idempotent turn it on.
//!
//! The caller enforces the matching property — a given `tool_use_id` is executed
//! **at most once** — so the same approved call can never be replayed into a
//! second real effect.
//!
//! # Honest limits (this milestone)
//!
//! The command runs as the user, with the user's environment, in the process's
//! working directory. There is **no sandbox**: approval is the boundary. Each
//! call is a fresh shell, so a `cd` in one call does not carry into the next
//! (chain it — `cd x && make` — instead).

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tracing::{Instrument, info, warn};

mod leftovers;
pub use leftovers::{LeftBehind, LeftProcess, Leftover, Leftovers};

/// Once the shell has exited with something still holding its pipes, how
/// long they are still read for the result. What the shell wrote is in the
/// pipe when it exits; this is time to read it. A bound, not "until quiet":
/// a leftover that never stops writing (`yes &`) never goes quiet, and the
/// read that waited for it never returned (probe, 2026-10-01).
const SETTLE: Duration = Duration::from_millis(100);

/// A deadline past any run: what a timeout too large for an `Instant` means
/// (tokio's own "far future" is thirty years).
const NEVER: Duration = Duration::from_secs(86_400 * 365 * 30);

/// Pause between retry attempts. Fixed rather than exponential: retries here are
/// a deliberate opt-in for commands the user knows are idempotent, not a
/// congestion-control problem — the point is only to stop an instantly-failing
/// command from becoming a spawn storm.
const RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// How a command is run. Plain data so the frontend can source it from config.
/// `PartialEq` but not `Eq`, following `SecretsConfig` — see there for why its
/// entropy threshold is a float.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecConfig {
    /// Start-to-close bound. On expiry the child is killed and the result says so.
    pub timeout: Duration,
    /// Cap on captured output, in bytes. Applied *while reading*, so a command
    /// that prints forever cannot exhaust memory before we notice.
    pub max_output: usize,
    /// Extra attempts after a failure. **`0` (the default) is off**; `-1` is
    /// unlimited.
    ///
    /// Off by default on purpose: a shell command is not assumed idempotent, and
    /// re-running one that *timed out* is the riskiest case of all — the timeout
    /// says it did not finish, not that it did nothing, so a retry can compound
    /// a partial effect. Opt in only for commands you know are safe to repeat.
    ///
    /// Note that unlimited retries and a command that always fails will keep a
    /// turn busy indefinitely (each attempt is still bounded by `timeout`). The
    /// natural companion is a total schedule-to-close cap; there is no knob for
    /// one yet, so treat `-1` as "I know this converges".
    pub max_retries: i32,
    /// Credential scrubbing applied to captured output **before it becomes an
    /// `ExecOutcome` at all** — so nothing downstream can leak what we never
    /// carried. Both sinks are covered by construction: the wire and the store
    /// both read the same already-scrubbed `output`.
    pub secrets: crate::SecretsConfig,
    /// The session's substitution table.
    ///
    /// Deliberately separate from [`secrets`](ExecConfig::secrets): that is
    /// serialisable policy and is written to disk, this holds live values and is
    /// neither serialised nor printed. **Secret** handles are resolved at the last
    /// possible moment (inside the attempt, after the gate) and the cleartext
    /// never travels back out — [`ExecOutcome::command`] keeps the handle form.
    /// Plain aliases were already resolved at the gate, so the human saw them.
    pub aliases: std::sync::Arc<crate::Aliases>,
    /// The shell used to interpret the command.
    ///
    /// **Must match what the analyzer models.** The permission engine
    /// understands bash — process substitution (`<(cmd)`) is bash, not POSIX sh
    /// — so running the line under a different shell would mean the analysis the
    /// human approved is not a description of what executes. That is a safety
    /// property, not a compatibility preference.
    pub shell: String,
    /// Where to keep an unredacted copy of output that was redacted, or `None`
    /// not to (the default). Set from `secrets.keep_redacted` by the app,
    /// which resolves the directory once (`Config::redacted_record_dir`) —
    /// the same resolver that puts it on the gate's protected list.
    pub keep_redacted_in: Option<std::path::PathBuf>,
    /// `[exec] env`, expanded ([`configured_env`]): laid over the minimal set
    /// a command is given, so `PATH` set here is the one it runs with.
    pub env: Vec<(String, String)>,
    /// What no command is handed, whatever named it ([`child_env`]): `[exec]
    /// withhold`, and every variable a secret was lent from.
    pub withheld: Vec<String>,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
            max_output: 64 * 1024,
            max_retries: 0,
            secrets: crate::SecretsConfig::default(),
            aliases: std::sync::Arc::new(crate::Aliases::default()),
            shell: "bash".to_string(),
            keep_redacted_in: None,
            env: Vec::new(),
            withheld: Vec::new(),
        }
    }
}

/// **`[exec] env`, as a command is given it** (Steven, 10-06: "be able to
/// set Shannon's `$PATH` and other variables. Default will be the user's
/// `$PATH`") — each `NAME = value` laid over the minimal set (`KEEP`);
/// `$NAME` and `${NAME}` in a value read from Shannon's own environment, so
/// `PATH = "$HOME/bin:$PATH"` extends yours and `SSH_AUTH_SOCK =
/// "$SSH_AUTH_SOCK"` passes yours on. The same for `claude`'s process.
///
/// And what to say, never silently: a name no environment holds (empty,
/// `=`, a NUL) is left out; so is a variable Shannon withholds — its key's
/// and the effort's (`claude_code::withheld_vars`), every one in `lent`, a
/// secret's (`Aliases::lent_vars`), and every one in `withhold` (`[exec]
/// withhold`, the user's own) — as a name, and as a `$NAME` in a value,
/// which is left empty there (`X = "$ANTHROPIC_API_KEY"` handed the key on
/// under another name: review 29). Handing on the key Shannon authenticates
/// with is what the minimal set exists to stop, and `[secrets]` is how a
/// command is given a value — through its handle, never its variable. A
/// `PATH` entry that is relative or empty is kept, and said: a command is
/// found there by the name your rules allow, so a file there runs as it.
pub fn configured_env(
    table: &std::collections::BTreeMap<String, String>,
    lent: &[String],
    withhold: &[String],
) -> (Vec<(String, String)>, Vec<String>) {
    // Whose a withheld variable is, said as why it was not handed on.
    let withheld = |var: &str| {
        if crate::backend::claude_code::withheld_vars().any(|w| w == var) {
            Some("Shannon's own")
        } else if lent.iter().any(|l| l == var) {
            Some("lent as a secret")
        } else if withhold.iter().any(|w| w == var) {
            Some("in [exec] withhold")
        } else {
            None
        }
    };
    let mut env = Vec::new();
    let mut said = Vec::new();
    for (name, value) in table {
        // A NUL ends a C string: `Command` would refuse the whole spawn.
        if name.is_empty() || name.contains(['=', '\0']) {
            said.push(format!(
                "[exec] env: `{name}` is no variable's name — left out"
            ));
            continue;
        }
        if value.contains('\0') {
            said.push(format!(
                "[exec] env: `{name}`'s value holds a NUL, which no variable can — left out"
            ));
            continue;
        }
        if let Some(whose) = withheld(name) {
            said.push(format!(
                "[exec] env: `{name}` is {whose} — never handed to a command \
                 (`[secrets]` gives a command a value)"
            ));
            continue;
        }
        let mut named = Vec::new();
        let value = expand(value, &mut |var| match withheld(var) {
            Some(whose) => {
                named.push((var.to_string(), whose));
                None
            }
            None => std::env::var(var).ok(),
        });
        for (var, whose) in named {
            said.push(format!(
                "[exec] env: `{name}` names `${var}`, {whose} — left empty there"
            ));
        }
        if name == "PATH" {
            for entry in value.split(':').filter(|e| !e.starts_with('/')) {
                let shown = if entry.is_empty() {
                    "an empty entry (the directory a command runs in)"
                } else {
                    entry
                };
                said.push(format!(
                    "[exec] env: PATH holds {shown} — a file found there runs under the name \
                     your rules allow"
                ));
            }
        }
        env.push((name.clone(), value));
    }
    (env, said)
}

/// `$NAME` and `${NAME}` in `value`, each as `lookup` has it (unset: empty);
/// a `$` before anything else is itself. A `${` never closed ends it: the
/// rest is kept as written — scanning on from each such `${` to the end
/// was quadratic in a value of them (review 29, Reviewer B).
fn expand(value: &str, lookup: &mut dyn FnMut(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, used) = match after.strip_prefix('{') {
            Some(braced) => match braced.find('}') {
                Some(end) => (&braced[..end], end + 2),
                None => {
                    out.push_str(&rest[at..]);
                    return out;
                }
            },
            None => {
                let end = after
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(after.len());
                (&after[..end], end)
            }
        };
        if name.is_empty() {
            out.push('$');
            rest = after;
            continue;
        }
        out.push_str(&lookup(name).unwrap_or_default());
        rest = &after[used..];
    }
    out.push_str(rest);
    out
}

/// **A program Shannon starts, found on Shannon's OWN `PATH`** — a name
/// with no `/` searched for there, an executable file; anything else (a
/// path, or a name found nowhere) as it is. A `Command` given a `PATH`
/// searches THAT one for its program, so `[exec] env`'s chose which shell
/// ran every command and which `claude` ran every turn (review 29,
/// Reviewer B). Both starters ask it: `attempt_once` (the shell) and
/// `claude_code::claude_command`.
pub(crate) fn on_own_path(program: impl AsRef<std::path::Path>) -> std::path::PathBuf {
    let program = program.as_ref();
    let own = std::env::var_os("PATH").unwrap_or_default();
    found_on(program, &own).unwrap_or_else(|| program.to_path_buf())
}

/// `program` as `path` finds it, as `execvp` would: the first executable
/// file of that name in its directories — `None` for a path (it holds a
/// `/`), or a name found nowhere.
fn found_on(program: &std::path::Path, path: &std::ffi::OsStr) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    if program.to_string_lossy().contains('/') {
        return None;
    }
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// What running a command produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutcome {
    /// The call this answers — the idempotency key, echoed so a late result can
    /// be matched to its request.
    pub tool_use_id: String,
    /// The command that produced this, **in handle form** — a secret substituted
    /// at execution is never written back here, so neither the transcript nor the
    /// store nor a log line can carry the cleartext.
    ///
    /// Carried so the transcript can say *which* command an output belongs to: an
    /// unattributed block of output following a DENIED command reads as though
    /// the denied command ran.
    pub command: String,
    /// Combined stdout and stderr, in arrival order, capped at `max_output`.
    /// Interleaved because that is how a human reads a terminal: a command whose
    /// error explains its output is unintelligible with the two separated.
    pub output: String,
    /// Exit status, or `None` if it was killed (timeout) or never started.
    pub code: Option<i32>,
    /// Whether the run hit the timeout and was killed.
    pub timed_out: bool,
    /// Whether output was cut off at the cap.
    pub truncated: bool,
    /// Set when the command could not be started at all — a missing shell, or a
    /// secret handle we could not resolve (which refuses rather than guesses).
    pub spawn_error: Option<String>,
    /// Credential kinds scrubbed from `output`, if any. Surfaced so the user is
    /// told and the model is not left reasoning about `[redacted: …]` as though
    /// the file literally contained that text.
    pub redacted: Vec<String>,
    /// Labels of the secrets substituted into this command. **Spending a secret
    /// is an event**, never a quiet one: this drives the user's notice and the
    /// note in the tool result.
    pub spent: Vec<String>,
    /// How many times it was actually run (1 unless retries are enabled).
    /// Surfaced so a repeated side effect is never invisible.
    pub attempts: u32,
    /// The unredacted copy of this output, when `secrets.keep_redacted` asked
    /// for one: `Ok(path)` where it was written, or `Err(why)` it could not be
    /// — so a failure reaches the user rather than leaving them believing a
    /// copy exists. `None` when nothing was redacted or no copy was asked for.
    /// **For the user only**: nothing here goes to the model.
    pub kept: Option<Result<String, String>>,
    /// What it left running when its shell exited (`server &`).
    pub left: LeftBehind,
    /// **What a file tool left** (`files`), `None` for a command. Its
    /// `command` is the call's subject (`Edit /work/a.rs`), its `output` the
    /// tool's own words, scrubbed, and its `code` 0: a file call has no exit
    /// status to report, and whether it failed is here.
    pub file: Option<FileRan>,
}

/// **What a file call left beside its words**: whether it was refused or
/// failed, its stamp for read-first, and where an Edit's replacements fell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRan {
    pub is_error: bool,
    pub stamp: Option<crate::files::Stamp>,
    pub places: Vec<crate::files::Place>,
}

/// **Run a file call and say what came of it** — `files::run`, its text
/// scrubbed as a command's output is (`Done::scrub`, under `cfg`'s secrets
/// and table), as the outcome every executor delivers. Blocking: file I/O.
///
/// **A secret its text names goes in here** (S21-Q27, `Call::spent`): at
/// the last moment, after the gate and the pane saw its handle, as a
/// command's does in `attempt_once`; spent and said (`spent`), or refused
/// for a handle not held or not lent. The job's subject keeps the handles.
pub fn run_file(tool_use_id: String, job: &crate::files::Job, cfg: &ExecConfig) -> ExecOutcome {
    let scrub = crate::files::Scrub {
        secrets: &cfg.secrets,
        aliases: &cfg.aliases,
    };
    let (done, spent) = match job.call.spent(&cfg.aliases) {
        Ok((call, spent)) => {
            let job = crate::files::Job {
                call,
                ..job.clone()
            };
            (job.run(scrub), spent)
        }
        Err(why) => (crate::files::Done::refused(why), Vec::new()),
    };
    let scrubbed = done.scrub(scrub);
    ExecOutcome {
        tool_use_id,
        command: job.subject(),
        output: scrubbed.text,
        code: Some(0),
        timed_out: false,
        truncated: false,
        spawn_error: None,
        redacted: scrubbed.redacted,
        spent,
        attempts: 1,
        kept: None,
        left: LeftBehind::Nothing,
        file: Some(FileRan {
            is_error: scrubbed.is_error,
            stamp: scrubbed.stamp,
            places: scrubbed.places,
        }),
    }
}

/// **Every file run ends in an outcome**: a call that ends with none holds
/// its turn forever, so a run that died says so, as one that failed does.
fn file_outcome(
    ran: Result<ExecOutcome, tokio::task::JoinError>,
    tool_use_id: String,
    command: String,
) -> ExecOutcome {
    ran.unwrap_or_else(|died| file_died(tool_use_id, command, &died))
}

/// The outcome of a file run that died before it said anything — it may
/// have written part of the file, and the model is told so.
fn file_died(tool_use_id: String, command: String, died: &tokio::task::JoinError) -> ExecOutcome {
    ExecOutcome {
        tool_use_id,
        command,
        output: format!(
            "The file tool stopped before it finished ({died}); the file may be part-written — read it before trusting it."
        ),
        code: Some(0),
        timed_out: false,
        truncated: false,
        spawn_error: None,
        redacted: Vec::new(),
        spent: Vec::new(),
        attempts: 1,
        kept: None,
        left: LeftBehind::Nothing,
        file: Some(FileRan {
            is_error: true,
            stamp: None,
            places: Vec::new(),
        }),
    }
}

impl ExecOutcome {
    /// The text handed back to the model: the output, plus a note whenever the
    /// run ended in a way the output alone would not reveal.
    ///
    /// Silence is ambiguous — "no output" and "killed after 120s" look identical
    /// otherwise, and a model that cannot tell them apart will draw a confident
    /// wrong conclusion.
    pub fn report(&self) -> String {
        if let Some(err) = &self.spawn_error {
            return format!("could not run the command: {err}");
        }
        let mut out = String::new();
        if self.output.trim().is_empty() {
            out.push_str("(no output)");
        } else {
            out.push_str(&self.output);
        }
        if self.truncated {
            out.push_str("\n[output truncated]");
        }
        if self.timed_out {
            out.push_str("\n[timed out and was killed — it may have done part of its work]");
        } else if let Some(code) = self.code
            && code != 0
        {
            out.push_str(&format!("\n[exit status {code}]"));
        }
        if !self.redacted.is_empty() {
            // Tell the MODEL too. Otherwise it reasons about `[redacted: …]` as
            // literal file contents and draws confident wrong conclusions — the
            // message-honesty rule, pointed at its third audience.
            // True whether or not the user keeps a copy, and says nothing
            // about one: a model told "the value is on the user's machine"
            // goes looking for it. It said exactly that, and it was false —
            // the output is scrubbed before anything records it.
            out.push_str(&format!(
                "\n[redacted before sending: {} — the value was removed before it reached you]",
                self.summary_of_redactions()
            ));
        }
        if !self.spent.is_empty() {
            // Same rule, other direction: the model wrote a handle and must know
            // the real value was substituted, or it will conclude the command ran
            // with a literal placeholder and "fix" a bug that isn't there.
            out.push_str(&format!(
                "\n[secret substituted at execution: {} — the value was used, not shown]",
                self.spent.join(", ")
            ));
        }
        if self.attempts > 1 {
            // A retried command may have had its effect more than once; that is
            // never something to leave implicit.
            out.push_str(&format!(
                "\n[ran {} times: retried on failure]",
                self.attempts
            ));
        }
        // Finished is not stopped: a model that started a server must know it
        // is still running, and that it will not outlive the session.
        match &self.left {
            LeftBehind::Nothing => {}
            // Held because its group could not be listed: "running" would be
            // a claim Shannon cannot make (Reviewer F, round 17).
            LeftBehind::Held(running) if running.is_empty() => out.push_str(
                "\n[it may have left something running — its process group could not be \
                 listed; Shannon ends it when it exits]",
            ),
            LeftBehind::Held(running) => out.push_str(&format!(
                "\n[still running after the command finished: {} — ended when Shannon exits]",
                processes(running)
            )),
            LeftBehind::Ended(running) => out.push_str(&format!(
                "\n[left running, and ended at once: {} — Shannon already holds the \
                 background processes of too many commands, which `:ls` lists]",
                processes(running)
            )),
            LeftBehind::Untracked => out.push_str(
                "\n[something it started still holds its output, where Shannon cannot see it \
                 — it may outlive Shannon]",
            ),
        }
        out
    }

    /// What was scrubbed from this outcome, counted by kind (`2 × github
    /// token, high-entropy string`) — a count so a short listing reads as short.
    pub fn summary_of_redactions(&self) -> String {
        crate::secrets::distinct(&self.redacted)
    }

    /// Whether this should be reported to the model as an error result — a
    /// file call's, as it said ([`FileRan::is_error`]).
    pub fn is_error(&self) -> bool {
        if let Some(file) = &self.file {
            return file.is_error;
        }
        self.spawn_error.is_some() || self.timed_out || self.code.is_none_or(|c| c != 0)
    }
}

/// `sleep (pid 4242), bash (pid 4241)`, the first few and a count of the rest.
pub fn processes(running: &[LeftProcess]) -> String {
    const SHOWN: usize = 4;
    // Held because its group could not be listed (`Membership::Unknown`).
    if running.is_empty() {
        return "processes it could not list (it may have ended)".into();
    }
    let mut named: Vec<String> = running
        .iter()
        .take(SHOWN)
        .map(|p| format!("{} (pid {})", p.name, p.pid))
        .collect();
    if running.len() > SHOWN {
        named.push(format!("{} more", running.len() - SHOWN));
    }
    named.join(", ")
}

/// Who actually runs a command.
///
/// A seam, for two reasons. **Safety:** a test suite must not be *able* to
/// execute a real destructive command — relying on "that path doesn't exist" or
/// "a rule denies it" makes `cargo test` one edit away from deleting the working
/// tree. Tests install a recorder and get the whole decision path with none of
/// the effect. **Growth:** the sandbox this milestone deliberately skips is
/// another implementation here, not a rewrite of everything above it.
pub trait Executor: Send + Sync + std::fmt::Debug {
    /// Start `command`, delivering its outcome on `tx`.
    ///
    /// Returns the task handle so the caller can **cancel** a running command
    /// (quit, `:new`, an abandoned round). Dropping a `JoinHandle` *detaches* the
    /// task rather than cancelling it, so a handle nobody keeps means a command
    /// nobody can stop — the caller must hold this and `abort()` it.
    fn spawn(
        &self,
        tool_use_id: String,
        command: String,
        cfg: ExecConfig,
        tx: UnboundedSender<ExecOutcome>,
    ) -> JoinHandle<()>;

    /// **Run a file tool's call** ([`run_file`]), delivering its outcome on
    /// `tx`. A seam for `spawn`'s reasons: a test must not be able to write
    /// a file through the tool path, and a sandbox later answers both. No
    /// default — each executor says what it does with one.
    fn file(
        &self,
        tool_use_id: String,
        job: crate::files::Job,
        cfg: ExecConfig,
        tx: UnboundedSender<ExecOutcome>,
    ) -> JoinHandle<()>;

    /// What finished commands left running (`server &`), oldest first.
    /// Nothing, for an executor that runs nothing.
    fn leftovers(&self) -> Vec<Leftover> {
        Vec::new()
    }

    /// End what finished commands left running: Shannon is leaving. Blocks
    /// briefly ([`Leftovers::end_all`]).
    fn end_leftovers(&self) {}
}

/// Runs commands for real, on this machine, as this user — and holds what
/// they leave running until it ends or Shannon does.
#[derive(Debug, Default, Clone)]
pub struct RealExecutor {
    leftovers: Leftovers,
}

impl Executor for RealExecutor {
    fn spawn(
        &self,
        tool_use_id: String,
        command: String,
        cfg: ExecConfig,
        tx: UnboundedSender<ExecOutcome>,
    ) -> JoinHandle<()> {
        spawn(tool_use_id, command, cfg, self.leftovers.clone(), tx)
    }

    /// Off the runtime's threads: a file read or written is blocking I/O,
    /// and one 64 MiB file would stall every task beside it.
    fn file(
        &self,
        tool_use_id: String,
        job: crate::files::Job,
        cfg: ExecConfig,
        tx: UnboundedSender<ExecOutcome>,
    ) -> JoinHandle<()> {
        tokio::spawn(
            async move {
                let (id, subject) = (tool_use_id.clone(), job.subject());
                let ran =
                    tokio::task::spawn_blocking(move || run_file(tool_use_id, &job, &cfg)).await;
                let _ = tx.send(file_outcome(ran, id, subject));
            }
            .instrument(tracing::Span::current()),
        )
    }

    fn leftovers(&self) -> Vec<Leftover> {
        self.leftovers.list()
    }

    fn end_leftovers(&self) {
        self.leftovers.end_all();
    }
}

/// Records what it was asked to run and reports a canned success — the executor
/// tests use, so the gate, the loop, and the transcript are all exercised while
/// nothing touches the machine.
#[derive(Debug, Default)]
pub struct RecordingExecutor {
    ran: std::sync::Mutex<Vec<String>>,
    jobs: std::sync::Mutex<Vec<crate::files::Job>>,
}

impl RecordingExecutor {
    /// Commands it was asked to run, in order.
    pub fn ran(&self) -> Vec<String> {
        self.ran.lock().map(|v| v.clone()).unwrap_or_default()
    }

    /// The file calls it was handed, in order — with what each was run
    /// under (its `context`: what the model's context held of the file).
    pub fn jobs(&self) -> Vec<crate::files::Job> {
        self.jobs.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

impl Executor for RecordingExecutor {
    fn spawn(
        &self,
        tool_use_id: String,
        command: String,
        _cfg: ExecConfig,
        tx: UnboundedSender<ExecOutcome>,
    ) -> JoinHandle<()> {
        if let Ok(mut v) = self.ran.lock() {
            v.push(command.clone());
        }
        let _ = tx.send(ExecOutcome {
            tool_use_id,
            command: command.clone(),
            output: format!("(recorded, not run) {command}"),
            code: Some(0),
            timed_out: false,
            truncated: false,
            spawn_error: None,
            redacted: Vec::new(),
            spent: Vec::new(),
            attempts: 1,
            kept: None,
            left: LeftBehind::Nothing,
            file: None,
        });
        tokio::spawn(async {})
    }

    /// Recorded by its subject (`Edit /work/a.rs`, and where it lands
    /// through a link) beside the commands, kept whole ([`jobs`](Self::jobs)),
    /// and answered as done — no file is opened. Its stamp names the job's
    /// file, so a later call's context finds it, and holds no file's hash:
    /// one never read can match none at a real run.
    fn file(
        &self,
        tool_use_id: String,
        job: crate::files::Job,
        _cfg: ExecConfig,
        tx: UnboundedSender<ExecOutcome>,
    ) -> JoinHandle<()> {
        let subject = job.subject();
        if let Ok(mut v) = self.ran.lock() {
            v.push(subject.clone());
        }
        let stamp = crate::files::Stamp {
            path: job.file().to_path_buf(),
            seen: crate::files::Lines::span(1, 1),
            total: 1,
            hash: [0; 32],
            shown: matches!(job.call, crate::files::Call::Read { .. }),
        };
        if let Ok(mut v) = self.jobs.lock() {
            v.push(job);
        }
        let _ = tx.send(ExecOutcome {
            tool_use_id,
            output: format!("(recorded, not run) {subject}"),
            command: subject,
            code: Some(0),
            timed_out: false,
            truncated: false,
            spawn_error: None,
            redacted: Vec::new(),
            spent: Vec::new(),
            attempts: 1,
            kept: None,
            left: LeftBehind::Nothing,
            file: Some(FileRan {
                is_error: false,
                stamp: Some(stamp),
                places: Vec::new(),
            }),
        });
        tokio::spawn(async {})
    }
}

/// An executor whose commands **never finish**, and which counts the ones
/// cancelled — the command-side twin of `HangingBackend`. A recorder's task
/// ends at once, so aborting its handle and dropping it (which DETACHES the
/// task) look alike: every `abort()` on the quit and stop paths could have
/// been a drop with the suite green (review stop 12, T3 1). The count is
/// kept by a guard inside each task, so it moves exactly when the future is
/// dropped — no sleeping, no margin.
#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Default)]
pub struct HangingExecutor {
    started: std::sync::atomic::AtomicUsize,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(any(test, feature = "test-util"))]
impl HangingExecutor {
    /// Commands started.
    pub fn started(&self) -> usize {
        self.started.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Commands whose task was cancelled (or whose runtime ended).
    pub fn cancelled(&self) -> usize {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Counts one cancellation when dropped — the task holding it was aborted.
#[cfg(any(test, feature = "test-util"))]
struct CountsCancel(std::sync::Arc<std::sync::atomic::AtomicUsize>);

#[cfg(any(test, feature = "test-util"))]
impl Drop for CountsCancel {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(any(test, feature = "test-util"))]
impl Executor for HangingExecutor {
    fn spawn(
        &self,
        _tool_use_id: String,
        _command: String,
        _cfg: ExecConfig,
        tx: UnboundedSender<ExecOutcome>,
    ) -> JoinHandle<()> {
        self.started
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let counts = CountsCancel(self.cancelled.clone());
        tokio::spawn(async move {
            // Held so no outcome can ever arrive: the command is running.
            let _tx = tx;
            let _counts = counts;
            std::future::pending::<()>().await;
        })
    }

    /// Never finishes either: a file call is cancelled as a command is.
    fn file(
        &self,
        tool_use_id: String,
        job: crate::files::Job,
        cfg: ExecConfig,
        tx: UnboundedSender<ExecOutcome>,
    ) -> JoinHandle<()> {
        self.spawn(tool_use_id, job.subject(), cfg, tx)
    }
}

/// Run `command` in the background, sending the outcome on `tx` when it ends.
///
/// Never blocks the caller: the UI stays live while a slow command runs, and the
/// outcome arrives as a message like any other.
///
/// **Hold the returned handle.** Dropping a `JoinHandle` detaches the task, so a
/// discarded handle is a command nothing can stop; `abort()` on it (plus the
/// child's `kill_on_drop` and its own process group) is what actually ends a run.
/// What it leaves running once finished goes to `leftovers`.
pub fn spawn(
    tool_use_id: String,
    command: String,
    cfg: ExecConfig,
    leftovers: Leftovers,
    tx: UnboundedSender<ExecOutcome>,
) -> JoinHandle<()> {
    // The caller's session span, carried into the task so "which conversation
    // ran this command" is answerable from the log. `ExecConfig` holds no
    // episode id on purpose — this crate runs commands and knows nothing about
    // sessions — so the attribution rides the span rather than a field.
    //
    // Read in this synchronous frame: a span entered inside the async block
    // would not survive the first `.await`.
    tokio::spawn(
        async move {
            let outcome = run(tool_use_id, &command, &cfg, &leftovers).await;
            let _ = tx.send(outcome);
        }
        .instrument(tracing::Span::current()),
    )
}

/// Run `command`, retrying on failure per [`ExecConfig::max_retries`].
///
/// Only *failures* are retried, and a success ends the loop immediately. Each
/// attempt gets the full `timeout`; the attempt count rides on the outcome so a
/// repeated side effect is visible rather than implied.
///
/// **Nor one that left something running**: its effect is still in progress,
/// the least idempotent state there is — and a second attempt's outcome
/// replaced the first's, so the model was told nothing ran while the first
/// attempt's server was held (Reviewers A, B, round 17).
async fn run(
    tool_use_id: String,
    command: &str,
    cfg: &ExecConfig,
    leftovers: &Leftovers,
) -> ExecOutcome {
    let mut attempts = 0u32;
    loop {
        let mut out = attempt_once(tool_use_id.clone(), command, cfg, leftovers).await;
        attempts += 1;
        out.attempts = attempts;
        let retries_left = cfg.max_retries < 0 || (attempts as i32) <= cfg.max_retries;
        if !out.is_error() || !retries_left || out.left != LeftBehind::Nothing {
            return out;
        }
        warn!(
            // Scrubbed like its four siblings in this file. `attempt_once` names
            // the reason in its own comment: an edited command is the likeliest
            // place a real credential is typed by hand — the one path where the
            // text is neither the model's nor scrubbed upstream — and it reaches
            // a 0644 log file. My last sweep counted `app.rs` only and claimed
            // the class closed; there are 13 such sites and this was the
            // thirteenth (Reviewer A, round 2).
            tool_use_id = %tool_use_id,
            command = %crate::secrets::redact(command, &cfg.secrets, &cfg.aliases).text,
            attempt = attempts,
            "command failed; retrying (retries are opt-in)"
        );
        // A pause between attempts. Without it, `max_retries = -1` on a command
        // that fails instantly (`false`, a missing binary) becomes a
        // process-spawn storm: fork/exec as fast as the OS allows, forever.
        tokio::time::sleep(RETRY_BACKOFF).await;
    }
}

/// One attempt: run the command to completion (or to its timeout).
async fn attempt_once(
    tool_use_id: String,
    command: &str,
    cfg: &ExecConfig,
    leftovers: &Leftovers,
) -> ExecOutcome {
    let mut base = ExecOutcome {
        tool_use_id,
        command: command.to_string(),
        output: String::new(),
        code: None,
        timed_out: false,
        truncated: false,
        spawn_error: None,
        redacted: Vec::new(),
        spent: Vec::new(),
        attempts: 1,
        kept: None,
        left: LeftBehind::Nothing,
        file: None,
    };
    // Scrub before logging. The log file is a durable, always-on sink, and an
    // *edited* command is the likeliest place a real credential is typed in by
    // hand — the one path where the text is neither the model's nor scrubbed
    // upstream. Handles are unaffected: this runs before expansion, so a secret
    // is still `{{…}}` here either way.
    info!(
        tool_use_id = %base.tool_use_id,
        command = %crate::secrets::redact(command, &cfg.secrets, &cfg.aliases).text,
        shell = %cfg.shell,
        "running approved command"
    );

    // Resolve HERE — after the gate, inside the attempt, at the last moment
    // before the shell sees the line. Two things land here, for opposite reasons:
    // a secret must never be displayed at all, and `~` is universally legible so
    // expanding it earlier would have traded a short portable path for a long
    // machine-specific one in front of the human. `base.command` keeps both in
    // their readable form, so the transcript, the store and the log all show what
    // the human approved. An unresolvable handle refuses the command rather than
    // guessing: expanding it to nothing would turn `rm -rf {{x}}/` into
    // `rm -rf /`.
    let resolved = match cfg.aliases.expand_at_exec(command) {
        Ok(e) => e,
        Err(why) => {
            warn!(
                tool_use_id = %base.tool_use_id,
                command = %crate::secrets::redact(command, &cfg.secrets, &cfg.aliases).text,
                "refusing to run: unresolvable secret handle"
            );
            base.spawn_error = Some(why);
            return base;
        }
    };
    base.spent = resolved.spent;

    // Found on Shannon's PATH: the child's own (`cfg.env`) would choose it.
    let mut child = tokio::process::Command::new(on_own_path(&cfg.shell));
    let child = child
        .arg("-c")
        .arg(&resolved.text)
        .stdin(Stdio::null()) // never inherit the TUI's stdin
        // Build the child's environment EXPLICITLY. Inheriting ours handed every
        // tool child the app's own credential (`ANTHROPIC_API_KEY`) — so a
        // read-only command, which policy runs without asking anyone, could echo
        // it straight into a `tool_result` and out to the API. A command should
        // not be able to read a secret it only had because *we* were holding it.
        .env_clear()
        // The minimal set, the hygiene defaults, then yours (`[exec] env`):
        // one table, built by the one door the gate reads too.
        .envs(child_env(&cfg.env, &cfg.withheld))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Lead a new process group so a timeout can signal EVERY process the command
    // started. Without this a pipeline's stages outlive the shell we killed.
    #[cfg(unix)]
    // tokio's Command exposes process_group directly on unix.
    let child = child.process_group(0);
    let child = child
        // Also tie the immediate child to the task, so a dropped/cancelled task
        // does not leave the shell itself running.
        .kill_on_drop(true)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            warn!(
                tool_use_id = %base.tool_use_id,
                command = %crate::secrets::redact(command, &cfg.secrets, &cfg.aliases).text,
                error = %e,
                shell = %cfg.shell,
                "could not spawn the shell",
            );
            base.spawn_error = Some(e.to_string());
            return base;
        }
    };
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    // **The command ends when its shell does**, not when its pipes close: a
    // `server &` that inherited them holds them open for as long as it runs,
    // so the call waited out its timeout and then killed the server (Reviewer
    // B, round 16, MED-2). The exit is seen WITHOUT reaping the shell, so its
    // pid — and the group it leads — stays ours while we look at what it left.
    let mut exited = shell_exit(child.id());
    // From here the shell is a `Leader`: dropped unreaped — a timeout, a stop,
    // Shannon exiting — it ends its whole group.
    let leader = Leader::new(child);
    let cap = cfg.max_output;
    // ONE shared buffer both readers append into, so stdout and stderr land in
    // ARRIVAL order. Concatenating them (all stdout, then all stderr) relocates
    // a compile error away from the line it explains — and the model reads this.
    //
    // It is also the buffer the timeout path reads, so a command that printed
    // 500 lines and then hung reports those 500 lines instead of "(no output)":
    // the case where partial output is *most* diagnostic is exactly the one the
    // old code discarded.
    let shared = std::sync::Mutex::new(Vec::<u8>::new());
    let truncated = std::sync::atomic::AtomicBool::new(false);

    // Read one pipe, appending until the cap and then DRAINING to EOF. Draining
    // matters: a child that fills a pipe nobody reads blocks forever, so the cap
    // must stop us storing, not stop us reading.
    //
    // The pipe is BORROWED, and `None` once it reaches EOF: a read cut short
    // at the shell's exit leaves it with us, still open, for whatever holds
    // its other end (`read` is cancel-safe, so no byte is lost).
    async fn drain<R: tokio::io::AsyncRead + Unpin>(
        pipe: &mut Option<R>,
        into: &std::sync::Mutex<Vec<u8>>,
        truncated: &std::sync::atomic::AtomicBool,
        cap: usize,
    ) {
        let Some(r) = pipe.as_mut() else {
            return;
        };
        let mut chunk = [0u8; 8192];
        loop {
            match r.read(&mut chunk).await {
                Ok(0) | Err(_) => {
                    *pipe = None;
                    return;
                }
                Ok(n) => {
                    if let Ok(mut buf) = into.lock() {
                        let room = cap.saturating_sub(buf.len());
                        if room == 0 {
                            truncated.store(true, std::sync::atomic::Ordering::Relaxed);
                        } else {
                            let take = room.min(n);
                            if take < n {
                                truncated.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            buf.extend_from_slice(&chunk[..take]);
                        }
                    }
                }
            }
        }
    }
    let pump = async |out: &mut Option<tokio::process::ChildStdout>,
                      err: &mut Option<tokio::process::ChildStderr>| {
        tokio::join!(
            drain(out, &shared, &truncated, cap),
            drain(err, &shared, &truncated, cap)
        );
    };

    let collect = async {
        tokio::select! {
            _ = pump(&mut stdout, &mut stderr) => (&mut exited).await.unwrap_or(None),
            exit = &mut exited => {
                let _ = tokio::time::timeout(SETTLE, pump(&mut stdout, &mut stderr)).await;
                exit.unwrap_or(None)
            }
        }
    };

    // One deadline for the whole run, the reap included. Added with a check:
    // `timeout_secs = 18446744073709551615` (a "never") overflowed `Instant`
    // and panicked every command — no outcome, the call "running" forever —
    // where the `timeout()` it replaced saturates (Reviewer B, round 17).
    let now = tokio::time::Instant::now();
    let deadline = now.checked_add(cfg.timeout).unwrap_or_else(|| now + NEVER);
    match tokio::time::timeout_at(deadline, collect).await {
        Ok(exit) => {
            // What it left running: anything of its group but its shell, which
            // is a zombie now and not counted. "Cannot tell" on a platform that
            // can is HELD, not released: a transient failure of the listing
            // let a live server go untracked (Reviewer D, round 17).
            use leftovers::Membership;
            let membership = leader
                .pgid()
                .map_or(Membership::Unsupported, leftovers::membership);
            let left = match membership {
                Membership::Live(live) if !live.is_empty() => Some(live),
                Membership::Unknown => Some(Vec::new()),
                Membership::Live(_) | Membership::Unsupported => None,
            };
            match left {
                Some(live) => {
                    base.code = exit;
                    let pipes = (stdout.take(), stderr.take());
                    let call = (base.tool_use_id.as_str(), base.command.as_str());
                    let printed =
                        leftovers::Printed::redacting(cfg.secrets.clone(), cfg.aliases.clone());
                    base.left = leftovers.hold(leader, call, live, pipes, printed);
                }
                None => {
                    // Nothing of it left, or no way to tell here: reap, as
                    // always. A reap cut short (a stop, the deadline) drops
                    // the leader unreaped, which ends the group.
                    match tokio::time::timeout_at(deadline, leader.reap()).await {
                        Ok(status) => base.code = status.ok().and_then(|s| s.code()),
                        Err(_) => base.timed_out = true,
                    }
                    // Yet something holds its output: it left the group, or
                    // this platform cannot list one. Read it slowly, so it is
                    // neither blocked nor killed by a closed pipe.
                    if stdout.is_some() || stderr.is_some() {
                        if let Some(pipe) = stdout.take() {
                            tokio::spawn(leftovers::drain_slowly(pipe, None));
                        }
                        if let Some(pipe) = stderr.take() {
                            tokio::spawn(leftovers::drain_slowly(pipe, None));
                        }
                        base.left = LeftBehind::Untracked;
                    }
                }
            }
        }
        Err(_) => base.timed_out = true,
    }
    if base.timed_out {
        // The leader is still unreaped, so the whole group dies as it drops —
        // here, or with the reap it was handed to: the door a stop goes
        // through.
        warn!(
            tool_use_id = %base.tool_use_id,
            command = %crate::secrets::redact(command, &cfg.secrets, &cfg.aliases).text,
            ?cfg.timeout,
            "command timed out; killing the process group"
        );
    }
    // Whatever was collected — including on the timeout path.
    let buf = shared.lock().map(|b| b.clone()).unwrap_or_default();
    base.truncated = truncated.load(std::sync::atomic::Ordering::Relaxed);
    let end = clamp_to_char_boundary(&buf, cap.min(buf.len()));
    let captured = String::from_utf8_lossy(&buf[..end]).into_owned();
    // Scrub HERE, before the outcome exists. Every consumer — the tool result
    // sent upstream, the transcript, the episode log on disk — reads this one
    // field, so there is no path that sees the cleartext.
    //
    // This is also the return leg for a spent secret: a command that expanded one
    // and then echoed it gets the value mapped back to its handle, so the round
    // trip closes and the model can act on the result without ever seeing it.
    let scrubbed = crate::secrets::redact(&captured, &cfg.secrets, &cfg.aliases);
    if !scrubbed.kinds.is_empty() {
        warn!(
            tool_use_id = %base.tool_use_id,
            kinds = %scrubbed.summary(),
            "redacted credentials from command output",
        );
        // The ONE place the cleartext exists, so the only place a copy can be
        // made. Opt-in (`secrets.keep_redacted`), and the result travels in
        // `kept`, which only the user's notice reads.
        if let Some(dir) = &cfg.keep_redacted_in {
            base.kept = Some(
                keep_unredacted(dir, &base.tool_use_id, &base.command, &captured)
                    .map(|p| p.display().to_string())
                    .map_err(|e| e.to_string()),
            );
        }
    }
    base.redacted = scrubbed.kinds;
    base.output = scrubbed.text;
    base
}

/// Write `raw` — output that redaction scrubbed — to its own owner-only file in
/// `dir`, returning the path.
///
/// **A plaintext file of secrets**, so every property that keeps it private is
/// enforced here rather than assumed:
/// - the directory is created `0700` and must be a real directory — a symlink
///   planted at that name is refused, not followed;
/// - each file is created `0600` with `create_new` (`O_EXCL`), so a
///   pre-existing file or symlink at the name is an error, never overwritten
///   or written through;
/// - one file per command, named by time and a sanitised call id, so nothing is
///   appended to a file someone else could have prepared.
fn keep_unredacted(
    dir: &std::path::Path,
    tool_use_id: &str,
    command: &str,
    raw: &str,
) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write as _;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    if !std::fs::symlink_metadata(dir)?.is_dir() {
        return Err(std::io::Error::other(format!(
            "{} is not a plain directory",
            dir.display()
        )));
    }
    let id: String = tool_use_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = dir.join(format!("{nanos}-{id}.txt"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&path)?;
    write!(
        file,
        "# Shannon: unredacted output (secrets.keep_redacted). Owner-only; delete when done.\n\
         # command: {command}\n\n{raw}"
    )?;
    Ok(path)
}

/// The names a tool child may inherit. Module-level so the test can assert the
/// **complement** — that nothing outside this list reaches a child — rather
/// than naming a few credentials it happens to think of.
const KEEP: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "TERM", "TMPDIR", "LANG", "LC_ALL", "LC_CTYPE",
    "TZ", "PWD",
];

/// **Hygiene: what Shannon sets so a command behaves as the gate read it.**
///
/// A pager is a program named by configuration — `git log` runs `$PAGER` or
/// `core.pager`, `gh` runs `$GH_PAGER` — so a read the gate allowed would run
/// a program it never saw; `cat` is the pager that is not one. The rest keep
/// a tool's output deterministic and its hands off the terminal: no update
/// nag, no interactive prompt, no colour escapes in a `tool_result`, no
/// optional lock files left by a `git status` (design
/// `docs/DesignDocs/permission-grammar.md` §9.11, Steven 2026-10-09: yes).
/// Each is a default under `[exec] env`, which wins — `PAGER = "less"` there
/// is the user's, and said nowhere.
const HYGIENE: &[(&str, &str)] = &[
    ("GIT_PAGER", "cat"),
    ("GH_PAGER", "cat"),
    ("PAGER", "cat"),
    ("NO_COLOR", "1"),
    ("GH_NO_UPDATE_NOTIFIER", "1"),
    ("GH_PROMPT_DISABLED", "1"),
    ("GIT_OPTIONAL_LOCKS", "0"),
];

/// **The environment a tool child is given**, built in one place: an
/// allowlist of ours (`KEEP`), then the hygiene defaults (`HYGIENE`), then
/// `[exec] env` — later wins, so a user's `PAGER` beats ours and an `[exec]
/// env` entry beats both — and none of `withheld` (`[exec] withhold`, a lent
/// secret's variable), whatever named it. Claude Code's `minimal` is this
/// too (`claude_code::claude_command`).
///
/// Enough for a normal command to behave (a `PATH` to find binaries, a `HOME`
/// for tool config, a locale for correct text), and nothing carrying our
/// credentials. Anything else a user needs is a knob we can add deliberately —
/// the default must not be "everything we happen to hold". The gate reads
/// the same table (`permission::acts`, the known-environment substitution):
/// what a command sees in `$HOME` or `$TMPDIR` is what Shannon put there, so
/// the gate may place a word that names them.
pub fn child_env(configured: &[(String, String)], withheld: &[String]) -> Vec<(String, String)> {
    let mut env: std::collections::BTreeMap<String, String> = KEEP
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect();
    for (name, value) in HYGIENE {
        env.insert(name.to_string(), value.to_string());
    }
    for (name, value) in configured {
        env.insert(name.clone(), value.clone());
    }
    env.into_iter()
        .filter(|(name, _)| !withheld.contains(name))
        .collect()
}

/// **A command's shell, unreaped, and the process group it leads — the one
/// thing that signals that group.**
///
/// The shell leads its own group (the `process_group(0)` at spawn), so one
/// signal to `-pgid` reaches every stage of a pipeline and anything it put in
/// the background. `kill_on_drop` alone signals only the shell, and the rest
/// outlived us, reparented and still working: first on timeout
/// (`find … | xargs …`), then on a stop (Steven, 2026-09-30: "ensure we are
/// closing everything when we close Shannon").
///
/// **The type is the invariant.** A pid that is not reaped is not reused, nor
/// the group it names, so while a `Leader` exists `-pgid` is this command's
/// and no one else's. Signalling takes a `&Leader`; [`reap`](Self::reap)
/// consumes it; one dropped unreaped — a timeout, a stop (`abort()` on the
/// task), a full registry, Shannon exiting — kills its group first, because
/// `Drop` runs before its fields drop. A finished command's leftovers are
/// held as their `Leader` (`exec/leftovers.rs`) until they end or Shannon
/// does. This replaced a guard whose order against the reap was held by
/// declaration order and a comment repeated at a dozen sites, with the pgid
/// copied out to a second signaller (Reviewers A, B, C, D, round 17).
#[derive(Debug)]
pub(super) struct Leader {
    child: tokio::process::Child,
    /// `None` only for a child already reaped at spawn, which has no group of
    /// ours to signal.
    pgid: Option<i32>,
    reaped: bool,
}

impl Leader {
    /// Lead `child`'s group — spawned with `process_group(0)`. Two spawners:
    /// a tool call's shell here, and a `claude -p` turn
    /// (`backend::claude_code`), whose abort must end Claude Code's own tool
    /// processes with it.
    pub(crate) fn new(child: tokio::process::Child) -> Self {
        let pgid = child.id().map(|p| p as i32);
        Self {
            child,
            pgid,
            reaped: false,
        }
    }

    /// Its group — for LISTING; only `signal` acts on it.
    pub(super) fn pgid(&self) -> Option<i32> {
        self.pgid
    }

    /// Signal the whole group.
    pub(super) fn signal(&self, signal: libc::c_int) {
        #[cfg(test)]
        if let (Some(pgid), Ok(mut sent)) = (self.pgid, SIGNALLED.lock()) {
            sent.push((pgid, signal));
        }
        #[cfg(unix)]
        if let Some(pgid) = self.pgid {
            // SAFETY: `kill(2)` with a negative pid signals the group. This
            // leader is unreaped (`reap` consumes it), so the pgid has not been
            // reused by another process.
            unsafe {
                libc::kill(-pgid, signal);
            }
        }
    }

    /// **Interrupt the leader alone** — `SIGINT` to its pid, not its group: a
    /// leader that ends its own work on an interrupt (`claude -p`, whose tool
    /// shells sit in groups of their own) is given the chance before anything
    /// signals the group under it.
    pub(crate) fn interrupt(&self) {
        #[cfg(unix)]
        if let Some(pid) = self.pgid {
            // SAFETY: the leader is unreaped, so its pid is still this child's.
            unsafe {
                libc::kill(pid, libc::SIGINT);
            }
        }
    }

    /// Reap the shell, which frees its group's number for anyone — so this
    /// consumes the leader. No `.await` between the wait and marking it
    /// reaped: a reap cut short drops an unreaped leader, which kills.
    pub(super) async fn reap(mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.wait().await;
        self.reaped = true;
        status
    }
}

impl Drop for Leader {
    fn drop(&mut self) {
        if !self.reaped {
            self.signal(libc::SIGKILL);
        }
    }
}

/// Every `(pgid, signal)` a `Leader` sent, in tests: "a reaped leader signals
/// nothing" cannot otherwise be seen — the number it would hit is free, and
/// a signal to a free group just fails.
#[cfg(test)]
static SIGNALLED: std::sync::Mutex<Vec<(i32, libc::c_int)>> = std::sync::Mutex::new(Vec::new());

/// The shell's exit, seen WITHOUT reaping it — `waitid(WNOWAIT)`, blocking,
/// on a thread of its own — with its exit code when it exited normally.
///
/// Unreaped, the shell is a zombie that keeps its pid, and so the group it
/// leads, from being reused while we look at what it left. The thread ends
/// when the shell does, which a timeout or a stop brings about by killing
/// the group.
fn shell_exit(pid: Option<u32>) -> JoinHandle<Option<i32>> {
    tokio::task::spawn_blocking(move || pid.and_then(exited_unreaped))
}

/// Block until `pid` has exited, leaving it unreaped. `None` for "no code":
/// killed by a signal — or the wait failed, which for our own unreaped child
/// does not happen; were it to, a shell still running would be listed as
/// running ([`leftovers`]), held, and ended with Shannon: never lost.
fn exited_unreaped(pid: u32) -> Option<i32> {
    // SAFETY: plain-old-data, filled by the kernel.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `info` is a valid `siginfo_t`; `WNOWAIT` leaves the child
        // waitable, so tokio's `Child::wait` still reaps it later.
        let r = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if r == 0 {
            return exit_code(&info);
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return None;
        }
    }
}

/// The exit code in `info`, if the child exited rather than being killed.
/// Read only where leftovers are held (macOS, Linux: where
/// `leftovers::membership` can list a group); elsewhere the shell is
/// reaped, and the code is read from that.
#[cfg(target_os = "macos")]
fn exit_code(info: &libc::siginfo_t) -> Option<i32> {
    // `<sys/signal.h>`; not in the `libc` crate for this target.
    const CLD_EXITED: libc::c_int = 1;
    (info.si_code == CLD_EXITED).then_some(info.si_status)
}

#[cfg(target_os = "linux")]
fn exit_code(info: &libc::siginfo_t) -> Option<i32> {
    // SAFETY: `waitid(WEXITED)` filled `info` for a child's exit, so its
    // union holds the `sigchld` fields `si_status` reads.
    (info.si_code == libc::CLD_EXITED).then(|| unsafe { info.si_status() })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn exit_code(_: &libc::siginfo_t) -> Option<i32> {
    None
}

/// Largest index `<= at` that is a UTF-8 char boundary, so truncating a byte
/// buffer can't split a multibyte character into mojibake.
fn clamp_to_char_boundary(buf: &[u8], at: usize) -> usize {
    let mut i = at.min(buf.len());
    // The end of the buffer is always a boundary; only *interior* continuation
    // bytes (`10xxxxxx`) mean we landed mid-character and must back off.
    while i > 0 && i < buf.len() && (buf[i] & 0b1100_0000) == 0b1000_0000 {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn run_cmd(cmd: &str, cfg: ExecConfig) -> ExecOutcome {
        run_held(cmd, cfg).await.0
    }

    /// Run `cmd`, with the registry its leftovers go to. The runtime's end
    /// drops every watcher, which ends what a test leaves held.
    async fn run_held(cmd: &str, cfg: ExecConfig) -> (ExecOutcome, Leftovers) {
        let leftovers = Leftovers::default();
        (run("t".to_string(), cmd, &cfg, &leftovers).await, leftovers)
    }

    #[tokio::test]
    async fn captures_output_and_exit_status() {
        let o = run_cmd("echo hello", ExecConfig::default()).await;
        assert!(o.output.contains("hello"), "{o:?}");
        assert_eq!(o.code, Some(0));
        assert!(!o.is_error());
        assert_eq!(o.tool_use_id, "t", "the result names the call it answers");
        // Its own exited shell is not "left running".
        assert_eq!(o.left, LeftBehind::Nothing);
    }

    /// **A reaped leader never signals its group; one dropped unreaped
    /// does.** The first is the whole point: once reaped, the group's number
    /// is free for anyone. That order was held by a field order and a dozen
    /// comments, and deleting the release passed every test (Reviewers A, B,
    /// C, D, round 17).
    #[tokio::test]
    async fn a_reaped_leader_never_signals_its_group() {
        let spawn = |args: &[&str]| {
            tokio::process::Command::new(args[0])
                .args(&args[1..])
                .process_group(0)
                .kill_on_drop(true)
                .spawn()
                .expect("spawns")
        };
        let sent = |pgid: i32| -> Vec<libc::c_int> {
            SIGNALLED
                .lock()
                .unwrap()
                .iter()
                .filter(|(p, _)| *p == pgid)
                .map(|(_, s)| *s)
                .collect()
        };
        let leader = Leader::new(spawn(&["true"]));
        let pgid = leader.pgid().expect("premise: a group");
        assert!(leader.reap().await.unwrap().success(), "premise: reaped");
        assert!(sent(pgid).is_empty(), "a reaped leader signalled its group");

        let leader = Leader::new(spawn(&["sleep", "30"]));
        let pgid = leader.pgid().expect("premise: a group");
        drop(leader);
        assert_eq!(sent(pgid), vec![libc::SIGKILL], "dropped unreaped: killed");
    }

    /// **A file call runs through the executor and says what it did** — the
    /// real one edits the file and answers in the tool's words, scrubbed, no
    /// exit status; a refusal is an error by `file`, not by a code; the
    /// recorder opens nothing.
    #[tokio::test]
    async fn a_file_call_runs_through_the_executor() {
        use crate::files::{Call, Context, Job};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "key=ghp_PLANTEDTOKENVALUE0123456789\nold\n").unwrap();
        // Read-first is the run's (`files::run`'s tests), not this seam's.
        let job = |call| Job {
            call,
            lands: None,
            context: Context {
                stamps: Vec::new(),
                first: crate::config::ReadFirst::Off,
            },
            caps: Default::default(),
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let run = |id: &str, call: Call| {
            RealExecutor::default().file(id.into(), job(call), ExecConfig::default(), tx.clone())
        };

        run(
            "t1",
            Call::Edit {
                path: path.clone(),
                old: "old".into(),
                new: "new".into(),
                all: false,
            },
        )
        .await
        .unwrap();
        let edited = rx.recv().await.unwrap();
        assert!(!edited.is_error(), "{edited:?}");
        assert_eq!(edited.command, format!("Edit {}", path.display()));
        assert!(
            edited.report().starts_with("Updated"),
            "{}",
            edited.report()
        );
        assert!(edited.file.as_ref().unwrap().stamp.is_some());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "key=ghp_PLANTEDTOKENVALUE0123456789\nnew\n"
        );

        run(
            "t2",
            Call::Read {
                path: path.clone(),
                offset: 1,
                limit: None,
            },
        )
        .await
        .unwrap();
        let read = rx.recv().await.unwrap();
        assert!(!read.output.contains("PLANTEDTOKEN"), "{}", read.output);
        assert!(
            read.report().contains("redacted before sending"),
            "{}",
            read.report()
        );

        run(
            "t3",
            Call::Read {
                path: dir.path().join("none"),
                offset: 1,
                limit: None,
            },
        )
        .await
        .unwrap();
        let missing = rx.recv().await.unwrap();
        assert!(missing.is_error());
        assert!(
            !missing.report().contains("exit status"),
            "{}",
            missing.report()
        );

        let recorder = RecordingExecutor::default();
        let made = dir.path().join("w.txt");
        let write = Call::Write {
            path: made.clone(),
            content: "x".into(),
        };
        recorder
            .file("t4".into(), job(write), ExecConfig::default(), tx.clone())
            .await
            .unwrap();
        assert!(!made.exists(), "the recorder wrote a file");
        assert_eq!(recorder.ran(), [format!("Write {}", made.display())]);
    }

    /// **A file call puts in the secrets its text names, at the run**
    /// (S21-Q27) — a Write's value in the file, spent and said, never in what
    /// the model is told; an Edit finds text by a handle (not spent: it
    /// writes nothing of it); a handle not held refuses, the file untouched.
    #[tokio::test]
    async fn a_file_call_puts_in_the_secrets_its_text_names() {
        use crate::files::{Call, Context, Job};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.env");
        let mut table = crate::Aliases::default();
        let handle = table
            .insert("db", "planted-db-value", "test", true)
            .unwrap();
        let cfg = ExecConfig {
            aliases: std::sync::Arc::new(table),
            ..ExecConfig::default()
        };
        let job = |call| Job {
            call,
            lands: None,
            context: Context {
                stamps: Vec::new(),
                first: crate::config::ReadFirst::Off,
            },
            caps: Default::default(),
        };
        let write = |content: String| Call::Write {
            path: path.clone(),
            content,
        };
        let done = run_file("t1".into(), &job(write(format!("KEY={handle}\n"))), &cfg);
        assert!(!done.is_error(), "{}", done.report());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "KEY=planted-db-value\n"
        );
        assert_eq!(done.spent, ["db"]);
        assert!(
            !done.report().contains("planted-db-value"),
            "{}",
            done.report()
        );
        assert!(done.command.contains("a.env") && !done.command.contains("planted"));

        let edit = Call::Edit {
            path: path.clone(),
            old: format!("KEY={handle}"),
            new: "KEY=other".into(),
            all: false,
        };
        let done = run_file("t2".into(), &job(edit), &cfg);
        assert!(!done.is_error(), "{}", done.report());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "KEY=other\n");
        assert!(
            done.spent.is_empty(),
            "found by it, not written: {:?}",
            done.spent
        );

        let unknown = handle.replace("db.", "dx.");
        let done = run_file("t3".into(), &job(write(format!("KEY={unknown}\n"))), &cfg);
        assert!(done.is_error() && done.report().contains("refusing to write the file"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "KEY=other\n",
            "untouched"
        );
    }

    /// **A held value read back is its handle, whatever its lines** (review
    /// stop 16, E 1, E's probe): a two-line value written by its handle,
    /// then Read, came back in clear — numbered, it was no longer one
    /// substring — and one at column 1,990 came back as its first ten
    /// characters, the 2,000-character cut inside it. Now the handle stands
    /// where the value starts, each later line says it continues, and the
    /// numbers are the file's.
    #[test]
    fn a_held_value_read_back_is_its_handle_whatever_its_lines() {
        use crate::files::{Call, Context, Job};
        let dir = tempfile::tempdir().unwrap();
        let mut table = crate::Aliases::default();
        let two = (table.insert(
            "db",
            "hunter2probeLineOne\nhunter2probeLineTwo",
            "test",
            true,
        ))
        .unwrap();
        table
            .insert("solo", "hunter2probeSolo", "test", true)
            .unwrap();
        let cfg = ExecConfig {
            aliases: std::sync::Arc::new(table),
            ..ExecConfig::default()
        };
        let job = |call| Job {
            call,
            lands: None,
            context: Context {
                stamps: Vec::new(),
                first: crate::config::ReadFirst::Off,
            },
            caps: Default::default(),
        };
        let read = |path: &std::path::Path| Call::Read {
            path: path.into(),
            offset: 1,
            limit: None,
        };
        let env = dir.path().join("a.env");
        let wrote = Call::Write {
            path: env.clone(),
            content: format!("PASS={two}\nnext\n"),
        };
        assert!(!run_file("w".into(), &job(wrote), &cfg).is_error());
        let done = run_file("r".into(), &job(read(&env)), &cfg);
        assert_eq!(
            done.output,
            format!("1\tPASS={two}\n2\t[… continued]\n3\tnext")
        );

        let long = dir.path().join("long.txt");
        std::fs::write(&long, format!("{}hunter2probeSolo\n", "x".repeat(1990))).unwrap();
        let done = run_file("l".into(), &job(read(&long)), &cfg);
        assert!(!done.is_error(), "{}", done.report());
        assert!(!done.output.contains("hunter2"), "{}", done.output);
    }

    /// **A file run that died still answers its call** — as an error that
    /// says the file may be part-written, for the call it was (a real
    /// `JoinError`, from a task that panicked).
    #[tokio::test]
    async fn a_file_run_that_died_still_answers() {
        let died = tokio::spawn(async { panic!("planted") }).await;
        let outcome = file_outcome(died.map(|()| unreachable!()), "t".into(), "Edit /a".into());
        assert!(outcome.is_error());
        assert_eq!(
            (outcome.tool_use_id.as_str(), outcome.command.as_str()),
            ("t", "Edit /a")
        );
        assert!(
            outcome.report().contains("part-written"),
            "{}",
            outcome.report()
        );
    }

    /// A group held because it could not be listed is not claimed to be
    /// running (Reviewer F, round 17).
    #[test]
    fn an_unlisted_group_is_not_said_to_run() {
        let outcome = ExecOutcome {
            tool_use_id: "t".into(),
            command: "server &".into(),
            output: String::new(),
            code: Some(0),
            timed_out: false,
            truncated: false,
            spawn_error: None,
            redacted: Vec::new(),
            spent: Vec::new(),
            attempts: 1,
            kept: None,
            left: LeftBehind::Held(Vec::new()),
            file: None,
        };
        let report = outcome.report();
        assert!(
            report.contains("may have left something running") && !report.contains("still running"),
            "{report}"
        );
    }

    /// A timeout too large for a clock — `u64::MAX` seconds, "never" — runs
    /// the command. It panicked every command, so none ever reported back
    /// and the call stayed "running" (Reviewer B, round 17).
    #[tokio::test]
    async fn a_timeout_of_never_still_runs_the_command() {
        let cfg = ExecConfig {
            timeout: Duration::from_secs(u64::MAX),
            ..ExecConfig::default()
        };
        let o = run_cmd("echo hello", cfg).await;
        assert!(o.output.contains("hello") && o.code == Some(0), "{o:?}");
    }

    /// stdout and stderr interleave: a command whose error explains its output
    /// is unintelligible with the two separated.
    #[tokio::test]
    async fn a_literal_credential_in_a_command_is_scrubbed_in_the_log() {
        // **A literal credential in a command never reaches the log** — the
        // lines that log the command, running it and retrying it, scrub it.
        // Deleting either scrub passed every test: they fed only handles,
        // which are expanded after the log (full review 10-21, E 1, mutated).
        //
        // One GLOBAL subscriber writing to this thread's sink, as the TUI's
        // `captured_logs` does: a scoped one (`set_default`) lost the lines
        // whose callsites another test's thread had cached as uninterested.
        use std::cell::RefCell;
        use std::sync::OnceLock;
        thread_local! {
            static CAPTURE: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
        }
        struct ThreadSink;
        impl std::io::Write for ThreadSink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                CAPTURE.with(|slot| {
                    if let Some(sink) = slot.borrow_mut().as_mut() {
                        sink.extend_from_slice(buf);
                    }
                });
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        static INSTALLED: OnceLock<()> = OnceLock::new();
        INSTALLED.get_or_init(|| {
            let subscriber = tracing_subscriber::fmt()
                .with_writer(|| ThreadSink)
                .with_ansi(false)
                .with_max_level(tracing::Level::TRACE)
                .finish();
            let _ = tracing::subscriber::set_global_default(subscriber);
        });
        CAPTURE.with(|slot| *slot.borrow_mut() = Some(Vec::new()));
        let cfg = ExecConfig {
            max_retries: 1,
            ..ExecConfig::default()
        };
        let o = run_cmd("echo password=hunter2seekrit; exit 1", cfg).await;
        let kept = CAPTURE
            .with(|slot| slot.borrow_mut().take())
            .unwrap_or_default();
        assert_eq!(o.attempts, 2, "premise: retried");
        let logs = String::from_utf8(kept).expect("utf-8");
        assert!(
            logs.contains("running approved command") && logs.contains("retrying"),
            "premise: both lines: {logs}"
        );
        assert!(!logs.contains("hunter2seekrit"), "{logs}");
    }

    #[tokio::test]
    async fn captures_stderr_and_a_failing_status() {
        let o = run_cmd("echo out; echo err 1>&2; exit 3", ExecConfig::default()).await;
        assert!(
            o.output.contains("out") && o.output.contains("err"),
            "{o:?}"
        );
        assert_eq!(o.code, Some(3));
        assert!(o.is_error(), "a non-zero exit is an error result");
        assert!(o.report().contains("exit status 3"));
    }

    /// A command that never ends must not hang the turn — and must actually be
    /// killed, not merely abandoned.
    #[tokio::test]
    async fn a_hanging_command_times_out() {
        let cfg = ExecConfig {
            timeout: Duration::from_millis(150),
            ..ExecConfig::default()
        };
        let o = run_cmd("sleep 30", cfg).await;
        assert!(o.timed_out);
        assert!(o.is_error());
        // The model must be able to tell "nothing happened" from "cut short".
        assert!(o.report().contains("timed out"), "{}", o.report());
    }

    /// Output is capped while reading, and the note says so — silence and
    /// truncation must not look alike.
    #[tokio::test]
    async fn output_is_capped_and_says_so() {
        let cfg = ExecConfig {
            max_output: 64,
            ..ExecConfig::default()
        };
        let o = run_cmd("head -c 5000 /dev/zero | tr '\\0' 'x'", cfg).await;
        assert!(o.truncated, "{o:?}");
        assert!(o.output.len() <= 64);
        assert!(o.report().contains("truncated"));
    }

    /// An empty result is stated, not left as ambiguous silence.
    #[tokio::test]
    async fn no_output_is_said_out_loud() {
        let o = run_cmd("true", ExecConfig::default()).await;
        assert_eq!(o.report(), "(no output)");
        assert!(!o.is_error());
    }

    /// The analyzer models bash (process substitution is bash, not POSIX sh), so
    /// the executor must run bash — otherwise the analysis a human approved is
    /// not a description of what runs.
    #[tokio::test]
    async fn bash_constructs_the_analyzer_accepts_actually_run() {
        let o = run_cmd(
            "cat <(echo via-process-substitution)",
            ExecConfig::default(),
        )
        .await;
        assert!(o.output.contains("via-process-substitution"), "{o:?}");
        assert_eq!(o.code, Some(0));
    }

    /// A missing shell is reported, not silently treated as an empty result.
    #[tokio::test]
    async fn an_unstartable_shell_is_reported() {
        let cfg = ExecConfig {
            shell: "definitely-not-a-shell-xyz".to_string(),
            ..ExecConfig::default()
        };
        let o = run_cmd("echo hi", cfg).await;
        assert!(o.spawn_error.is_some());
        assert!(o.is_error());
        assert!(o.report().contains("could not run"));
    }

    /// Retries are OFF by default: a failing command runs exactly once, because
    /// a shell command is not assumed idempotent.
    #[tokio::test]
    async fn a_failure_is_not_retried_by_default() {
        let o = run_cmd("exit 1", ExecConfig::default()).await;
        assert_eq!(o.attempts, 1, "no retry unless asked for");
        assert!(o.is_error());
    }

    /// Opted in, a failing command is retried up to the limit — and the report
    /// says how many times it ran, so a repeated side effect is never invisible.
    #[tokio::test]
    async fn retries_are_bounded_and_visible_when_enabled() {
        let cfg = ExecConfig {
            max_retries: 2,
            ..ExecConfig::default()
        };
        let o = run_cmd("exit 1", cfg).await;
        assert_eq!(o.attempts, 3, "the original plus two retries");
        assert!(o.report().contains("ran 3 times"), "{}", o.report());
    }

    /// A success ends the loop immediately — retries never re-run something that
    /// already worked.
    #[tokio::test]
    async fn success_is_never_retried() {
        let cfg = ExecConfig {
            max_retries: 5,
            ..ExecConfig::default()
        };
        let o = run_cmd("echo ok", cfg).await;
        assert_eq!(o.attempts, 1);
        assert!(!o.report().contains("ran"), "no retry note on a clean run");
    }

    /// A command that prints forever must be capped **while reading**, not after
    /// — the doc claimed this and the code did the opposite (`read_to_end` then
    /// truncate), so `yes` under the default timeout could exhaust memory long
    /// before the 64 KiB cap was ever consulted.
    #[tokio::test]
    async fn a_firehose_is_capped_while_reading() {
        let cfg = ExecConfig {
            max_output: 4096,
            timeout: Duration::from_millis(400),
            ..ExecConfig::default()
        };
        // Unbounded output: only a cap applied during the read can contain it.
        let o = run_cmd("yes hello", cfg).await;
        assert!(o.truncated, "{o:?}");
        assert!(
            o.output.len() <= 4096,
            "held at the cap, not grown to the timeout: {}",
            o.output.len()
        );
    }

    /// stdout and stderr must interleave in ARRIVAL order. Concatenating them
    /// relocates an error away from the line it explains — and the model reads
    /// this output to decide what to do next.
    #[tokio::test]
    async fn streams_interleave_in_arrival_order() {
        let o = run_cmd(
            "echo first; sleep 0.05; echo second 1>&2; sleep 0.05; echo third",
            ExecConfig::default(),
        )
        .await;
        let (a, b, c) = (
            o.output.find("first"),
            o.output.find("second"),
            o.output.find("third"),
        );
        assert!(
            a < b && b < c,
            "arrival order preserved, not stdout-then-stderr: {:?}",
            o.output
        );
    }

    /// A command that printed and *then* hung must report what it printed. The
    /// timeout path used to drop the buffer, so the most diagnostic case — "it
    /// got this far, then stopped" — reported `(no output)`.
    #[tokio::test]
    async fn a_timeout_keeps_the_output_produced_so_far() {
        let cfg = ExecConfig {
            timeout: Duration::from_millis(300),
            ..ExecConfig::default()
        };
        let o = run_cmd("echo progress-so-far; sleep 30", cfg).await;
        assert!(o.timed_out);
        assert!(
            o.output.contains("progress-so-far"),
            "partial output survives the kill: {:?}",
            o.output
        );
        assert!(o.report().contains("timed out"));
    }

    /// A two-stage pipeline whose stages write their own pids to `pids`, then
    /// `exec` into a long sleep, so each recorded pid IS a running stage.
    ///
    /// Survivors are found by pid with `kill(pid, 0)`, not by `pgrep`: under
    /// the macOS sandbox `pgrep` cannot list processes, and a test that read
    /// its empty output as "nothing survived" passed whatever happened.
    fn recording_pipeline(pids: &std::path::Path) -> String {
        let stage = format!("sh -c 'echo $$ >> {}; exec sleep 30'", pids.display());
        format!("{stage} | {stage}")
    }

    fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 only checks the pid exists and may be signalled.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// **`while` the process `pid` lives** — the loop a fixture that never
    /// ends on its own runs instead of `while :`, watching the test process.
    ///
    /// A mutant that stopped signalling failed
    /// `exit_asks_leftovers_to_end_then_kills_the_ones_that_wont` — correctly
    /// — and its two `while :` loops ran on for eighteen hours (Steven found
    /// them, 2026-10-01). Bounded by the test PROCESS rather than by cleanup
    /// code, because a test can fail, hang, or be SIGKILLed by a harness
    /// timeout, and in the last two no cleanup of ours runs; the process
    /// going away is the one event every ending shares. (`yes &` needs none:
    /// it dies of SIGPIPE once the test process, its reader, is gone. A
    /// `sleep 30` ends by itself.)
    fn while_alive(pid: u32) -> String {
        format!("while kill -0 {pid} 2>/dev/null")
    }

    /// The bound holds: a loop watching a process that is gone ends at once,
    /// even ignoring SIGTERM; one watching a live process runs on.
    #[test]
    fn a_fixture_loop_ends_with_the_process_it_watches() {
        let looping = |pid: u32| {
            std::process::Command::new("sh")
                .args([
                    "-c",
                    &format!("trap '' TERM; {}; do sleep 0.05; done", while_alive(pid)),
                ])
                .spawn()
                .expect("sh")
        };
        let mut gone = std::process::Command::new("true").spawn().expect("true");
        let _ = gone.wait(); // reaped: its pid names nothing now
        let mut orphaned = looping(gone.id());
        let ended = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(20));
            orphaned.try_wait().expect("try_wait").is_some()
        });
        // Killed BEFORE the verdict: if the bound is broken, this test's own
        // fixture is the one that would run on forever.
        let _ = orphaned.kill();
        let _ = orphaned.wait();
        assert!(ended, "a loop whose process is gone ends");

        let mut watching_us = looping(std::process::id());
        std::thread::sleep(Duration::from_millis(200));
        let running = watching_us.try_wait().expect("try_wait").is_none();
        let _ = watching_us.kill();
        let _ = watching_us.wait();
        assert!(running, "premise: it runs while the test process does");
    }

    /// The stages' pids, once both have started (premise: both are alive).
    async fn started_stages(pids: &std::path::Path) -> Vec<i32> {
        for _ in 0..100 {
            let read: Vec<i32> = std::fs::read_to_string(pids)
                .unwrap_or_default()
                .lines()
                .filter_map(|l| l.trim().parse().ok())
                .collect();
            if read.len() == 2 {
                assert!(
                    read.iter().all(|&p| alive(p)),
                    "premise: the stages are running"
                );
                return read;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("premise: the pipeline's stages never started");
    }

    /// Every stage gone, after a moment for the signal to land.
    async fn assert_none_survive(stages: &[i32]) {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let survivors: Vec<_> = stages.iter().filter(|&&p| alive(p)).collect();
        assert!(
            survivors.is_empty(),
            "the whole process group is gone; survivors: {survivors:?}"
        );
    }

    /// A timed-out PIPELINE must not leave its stages running. `kill_on_drop`
    /// signals only the shell, so `find … | xargs …` — the commonest shape an
    /// agent runs — used to outlive the command we reported dead.
    #[tokio::test]
    async fn a_timed_out_pipeline_leaves_nothing_running() {
        let tmp = tempfile::tempdir().unwrap();
        let pids = tmp.path().join("pids");
        let cfg = ExecConfig {
            timeout: Duration::from_millis(500),
            ..ExecConfig::default()
        };
        let command = recording_pipeline(&pids);
        let (o, stages) = tokio::join!(run_cmd(&command, cfg), started_stages(&pids));
        assert!(o.timed_out);
        assert_none_survive(&stages).await;
    }

    /// A STOPPED pipeline must not leave its stages running either: aborting
    /// the task (CTRL-C, or Shannon exiting, which drops every task) killed
    /// only the shell, through `kill_on_drop`.
    #[tokio::test]
    async fn an_aborted_pipeline_leaves_nothing_running() {
        let tmp = tempfile::tempdir().unwrap();
        let pids = tmp.path().join("pids");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = spawn(
            "t".into(),
            recording_pipeline(&pids),
            ExecConfig::default(),
            Leftovers::default(),
            tx,
        );
        let stages = started_stages(&pids).await;
        handle.abort();
        let _ = handle.await;
        assert!(rx.try_recv().is_err(), "an aborted run reports nothing");
        assert_none_survive(&stages).await;
    }

    // --- leftovers: what a finished command left running -------------------

    /// **Held where a group can be listed** (`leftovers::membership`: macOS,
    /// Linux) — one gate for every test that needs a leftover held, so a
    /// platform added there is added here once.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    mod held {
        use super::*;

        /// The pid a command printed on its first line (`… & echo $!`).
        fn first_pid(o: &ExecOutcome) -> i32 {
            o.output
                .lines()
                .next()
                .and_then(|l| l.trim().parse().ok())
                .unwrap_or_else(|| panic!("premise: a pid on the first line: {o:?}"))
        }

        /// Gone within a second: a killed leftover is reparented to init, which
        /// reaps it.
        async fn gone_soon(pid: i32) -> bool {
            for _ in 0..50 {
                if !alive(pid) {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            false
        }

        /// **A background child does not hold the call, even holding its
        /// output**: the call ends with the shell, keeps what the shell wrote, and
        /// says what is still running. A `server &` that inherited the pipes held
        /// them open, so the call waited out its whole timeout and then killed the
        /// server (Reviewer B, round 16, MED-2).
        #[tokio::test]
        async fn a_background_child_holding_the_output_does_not_hold_the_call() {
            let cfg = ExecConfig {
                timeout: Duration::from_secs(20),
                ..ExecConfig::default()
            };
            let started = std::time::Instant::now();
            let (o, leftovers) = run_held("sleep 30 & echo $!; echo done", cfg).await;
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "returned at the shell's exit, not the pipe's: {:?}",
                started.elapsed()
            );
            assert!(!o.timed_out && o.code == Some(0), "{o:?}");
            assert!(
                o.output.contains("done"),
                "what the shell wrote is kept: {o:?}"
            );
            let pid = first_pid(&o);
            assert_eq!(
                o.left,
                LeftBehind::Held(vec![LeftProcess {
                    pid,
                    name: "sleep".into()
                }])
            );
            let report = o.report();
            assert!(
                report.contains(&format!("sleep (pid {pid})"))
                    && report.contains("ended when Shannon exits"),
                "the model is told: {report}"
            );
            let listed = leftovers.list();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].command, "sleep 30 & echo $!; echo done");
            assert_eq!(listed[0].tool_use_id, "t", "the call that started it");
            assert!(alive(pid), "held, not killed");
            leftovers.end_all();
            assert!(gone_soon(pid).await, "ended when Shannon exits");
        }

        /// A FINISHED run's background child, its output redirected, is HELD: it
        /// keeps running, it is listed, and Shannon ends it on the way out. It
        /// was released with the shell, and outlived Shannon (Steven, 2026-09-30:
        /// "clarity into what is actually running").
        #[tokio::test]
        async fn a_finished_runs_background_child_is_held_until_shannon_exits() {
            let (o, leftovers) =
                run_held("sleep 30 >/dev/null 2>&1 & echo $!", ExecConfig::default()).await;
            let pid = first_pid(&o);
            assert!(
                matches!(&o.left, LeftBehind::Held(p) if p.iter().any(|p| p.pid == pid)),
                "{o:?}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert!(alive(pid), "a finished run's background child was killed");
            assert_eq!(leftovers.list()[0].processes.len(), 1);
            leftovers.end_all();
            assert!(gone_soon(pid).await, "ended when Shannon exits");
        }

        /// A leftover that ends on its own is let go: forgotten within a poll, so
        /// nothing goes on naming it, and its shell is reaped.
        #[tokio::test]
        async fn a_leftover_that_ends_is_let_go() {
            let (o, leftovers) =
                run_held("sleep 0.3 >/dev/null 2>&1 &", ExecConfig::default()).await;
            assert!(matches!(o.left, LeftBehind::Held(_)), "premise: {o:?}");
            for _ in 0..100 {
                if leftovers.list().is_empty() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("still listed after it ended: {:?}", leftovers.list());
        }

        /// A failure that left something running is not retried: the model would
        /// be told about the last attempt only, with the first one's server held.
        #[tokio::test]
        async fn a_failure_that_left_something_running_is_not_retried() {
            let cfg = ExecConfig {
                max_retries: 2,
                ..ExecConfig::default()
            };
            let (o, leftovers) = run_held("sleep 30 >/dev/null 2>&1 & exit 1", cfg).await;
            assert_eq!(o.attempts, 1, "{o:?}");
            assert!(matches!(o.left, LeftBehind::Held(_)), "{o:?}");
            assert_eq!(leftovers.list().len(), 1, "one attempt, one held");
            leftovers.end_all();
        }

        /// The status is the shell's own, read while it is held unreaped — and a
        /// shell killed by a signal has none, held or not.
        #[tokio::test]
        async fn a_held_commands_exit_status_is_its_shells() {
            let (o, leftovers) =
                run_held("sleep 30 >/dev/null 2>&1 & exit 3", ExecConfig::default()).await;
            assert!(matches!(o.left, LeftBehind::Held(_)), "premise: {o:?}");
            assert_eq!(o.code, Some(3));
            leftovers.end_all();

            let (o, leftovers) = run_held(
                "sleep 30 >/dev/null 2>&1 & kill -KILL $$",
                ExecConfig::default(),
            )
            .await;
            assert!(matches!(o.left, LeftBehind::Held(_)), "premise: {o:?}");
            assert_eq!(o.code, None, "killed, not exited");
            leftovers.end_all();
        }

        /// **Asked, then made**: a leftover gets SIGTERM first, so a server can
        /// clean up after itself, and one that ignores it is killed after the
        /// grace.
        #[tokio::test]
        async fn exit_asks_leftovers_to_end_then_kills_the_ones_that_wont() {
            let tmp = tempfile::tempdir().unwrap();
            let said = tmp.path().join("said");
            // Each says when its trap is set, and the command waits for that: a
            // TERM sent sooner ended the "stubborn" one before it could ignore
            // anything, and this test passed with the SIGKILL deleted.
            let leftover = |trap: &str, ready: &std::path::Path| {
                format!(
                    "sh -c 'trap {trap} TERM; : > {ready}; {}; do sleep 0.05; done' \
                 >/dev/null 2>&1 & echo $!; while [ ! -e {ready} ]; do sleep 0.01; done",
                    while_alive(std::process::id()),
                    ready = ready.display()
                )
            };
            let polite = leftover(
                &format!("\"echo term > {}; exit 0\"", said.display()),
                &tmp.path().join("ready-a"),
            );
            let stubborn = leftover("\"\"", &tmp.path().join("ready-b"));
            let leftovers = Leftovers::default();
            let cfg = ExecConfig::default();
            let a = run("a".into(), &polite, &cfg, &leftovers).await;
            let b = run("b".into(), &stubborn, &cfg, &leftovers).await;
            let (pa, pb) = (first_pid(&a), first_pid(&b));
            assert_eq!(leftovers.list().len(), 2, "premise: both held");
            let started = std::time::Instant::now();
            leftovers.end_all();
            assert!(
                started.elapsed() >= Duration::from_millis(900),
                "premise: one ignored SIGTERM for the whole grace ({:?})",
                started.elapsed()
            );
            assert!(gone_soon(pa).await && gone_soon(pb).await, "both ended");
            assert_eq!(
                std::fs::read_to_string(&said).unwrap_or_default(),
                "term\n",
                "asked to end before it was made to"
            );
        }

        /// Ending takes as long as the leftovers do, not the whole grace.
        #[tokio::test]
        async fn ending_leftovers_waits_only_as_long_as_they_take() {
            let (o, leftovers) =
                run_held("sleep 30 >/dev/null 2>&1 & echo $!", ExecConfig::default()).await;
            // The premise: with nothing held, `end_all` returns at once and the
            // bound below passes whatever it guards (Reviewer C, round 17).
            assert!(matches!(o.left, LeftBehind::Held(_)), "premise: {o:?}");
            assert_eq!(leftovers.list().len(), 1, "premise: held");
            let started = std::time::Instant::now();
            leftovers.end_all();
            assert!(
                started.elapsed() < Duration::from_millis(600),
                "{:?}",
                started.elapsed()
            );
        }

        /// **A leftover that never stops writing** does not hold the call (it
        /// never goes quiet, so the settle has a deadline: without one the call
        /// never returned), is not killed by a closed pipe, and is read slowly —
        /// `yes &` read flat out cost a whole core.
        #[tokio::test]
        async fn a_leftover_writing_nonstop_runs_on_and_is_read_slowly() {
            let cfg = ExecConfig {
                timeout: Duration::from_secs(10),
                ..ExecConfig::default()
            };
            let started = std::time::Instant::now();
            let (o, leftovers) = run_held("yes &", cfg).await;
            assert!(
                started.elapsed() < Duration::from_secs(3) && !o.timed_out,
                "returned: {:?}, timed out: {}",
                started.elapsed(),
                o.timed_out
            );
            let LeftBehind::Held(running) = &o.left else {
                panic!("premise: held, got {:?}", o.left);
            };
            let pid = running[0].pid;
            let before = leftovers.list()[0].printed;
            tokio::time::sleep(Duration::from_secs(1)).await;
            let read = leftovers.list()[0].printed - before;
            assert!(alive(pid), "killed by a closed pipe");
            assert!(read > 0, "its output is still read");
            assert_eq!(
                leftovers.list()[0].last_line.as_deref(),
                Some("y"),
                "what it said last"
            );
            assert!(
                read <= 3 * leftovers::DRAIN_RATE,
                "read slowly: {read} bytes in a second"
            );
            leftovers.end_all();
        }

        /// Past the cap, a command's leftovers are ended at once, and its result
        /// says so — not held without bound, not released unseen.
        #[tokio::test]
        async fn past_the_cap_leftovers_are_ended_and_said_to_be() {
            let leftovers = Leftovers::with_cap(1);
            let cfg = ExecConfig::default();
            let cmd = "sleep 30 >/dev/null 2>&1 & echo $!";
            let a = run("a".into(), cmd, &cfg, &leftovers).await;
            let b = run("b".into(), cmd, &cfg, &leftovers).await;
            assert!(matches!(a.left, LeftBehind::Held(_)), "premise: {a:?}");
            assert!(matches!(b.left, LeftBehind::Ended(_)), "{b:?}");
            assert!(b.report().contains("ended at once"), "{}", b.report());
            assert!(gone_soon(first_pid(&b)).await, "ended");
            assert!(alive(first_pid(&a)), "the held one runs on");
            assert_eq!(leftovers.list().len(), 1);
            leftovers.end_all();
        }

        /// **The runtime's end ends what it held**, whatever way out skipped
        /// `end_all`: each leader is dropped unreaped, which ends its group —
        /// the door a timeout and a stop go through.
        ///
        /// As in Shannon, the registry's last handles are the app's (gone when
        /// `block_on` returns, here) and its watchers' (gone with the runtime);
        /// its leaders go with it, unreaped, and each ends its group.
        #[test]
        fn a_dropped_runtime_ends_what_it_held() {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            let pid = rt.block_on(async {
                let leftovers = Leftovers::default();
                let cmd = "sleep 30 >/dev/null 2>&1 & echo $!";
                let o = run("t".into(), cmd, &ExecConfig::default(), &leftovers).await;
                assert!(matches!(o.left, LeftBehind::Held(_)), "premise: {o:?}");
                first_pid(&o)
            });
            assert!(alive(pid), "premise: held while its watcher lives");
            drop(rt);
            for _ in 0..50 {
                if !alive(pid) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("pid {pid} outlived the runtime that held it");
        }
    }

    /// Something that LEFT the command's process group but holds its output
    /// is said to exist — Shannon cannot track or end it — the call does not
    /// wait for it, and its output is still read, so it does not die of a
    /// closed pipe at its next line.
    #[tokio::test]
    async fn a_process_that_left_the_group_is_said_to_hold_the_output() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        // It signals once it has a session of its own, so the shell exits
        // after; then it writes a line every 50ms.
        let command = format!(
            "perl -MPOSIX -e 'POSIX::setsid(); open(F, \">\", $ARGV[0]); close(F); $| = 1; \
             while (1) {{ print \"x\\n\"; select(undef, undef, undef, 0.05) }}' {left} & \
             echo pid=$!; while [ ! -e {left} ]; do sleep 0.01; done",
            left = left.display()
        );
        let cfg = ExecConfig {
            timeout: Duration::from_secs(20),
            ..ExecConfig::default()
        };
        let started = std::time::Instant::now();
        let o = run_cmd(&command, cfg).await;
        let elapsed = started.elapsed();
        let pid: i32 = o
            .output
            .lines()
            .find_map(|l| l.strip_prefix("pid=")?.trim().parse().ok())
            .unwrap_or_else(|| panic!("premise: its pid: {o:?}"));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let still_writing = alive(pid);
        // SAFETY: the pid is the test's own escaped child; cleaning up.
        unsafe { libc::kill(pid, libc::SIGKILL) };
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
        assert_eq!(o.left, LeftBehind::Untracked, "{o:?}");
        assert!(o.report().contains("may outlive Shannon"), "{}", o.report());
        assert!(still_writing, "killed by a closed pipe");
    }

    /// **The planted-key test.** A real credential in a real file, read by a
    /// real command, through the real pipeline: it must not survive into the
    /// outcome the model is sent or the store persists.
    ///
    /// Written as an end-to-end test on purpose — unit-testing `redact` proves
    /// the scrubber works, not that it is actually *on the path*. The bug this
    /// guards is "we built a redactor and forgot to call it".
    #[tokio::test]
    async fn a_planted_key_never_reaches_the_outcome() {
        let dir = std::env::temp_dir().join(format!("shannon-planted-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("creds");
        let key = "sk-ant-api03-PLANTEDKEYVALUE0123456789";
        std::fs::write(&file, format!("ANTHROPIC_API_KEY={key}\n")).expect("write");

        let o = run_cmd(&format!("cat {}", file.display()), ExecConfig::default()).await;
        std::fs::remove_dir_all(&dir).ok();

        assert!(
            !o.output.contains(key),
            "the key reached ExecOutcome.output: {:?}",
            o.output
        );
        assert!(
            !o.report().contains(key),
            "the key reached the text sent to the model: {:?}",
            o.report()
        );
        // The surrounding line survives, so the model still learns the shape of
        // the file — which is the whole point of §7's keys-visible policy.
        assert!(o.output.contains("ANTHROPIC_API_KEY"), "{:?}", o.output);
        assert!(o.redacted.iter().any(|k| k == "anthropic key"), "{o:?}");
        // And the model is TOLD, so it doesn't reason about the placeholder as
        // literal file contents.
        assert!(
            o.report().contains("redacted before sending"),
            "{}",
            o.report()
        );
    }

    /// Redaction is on the capture path, so turning it off is the only way to
    /// see cleartext — proving the default is what protects you.
    #[tokio::test]
    async fn disabling_redaction_is_the_only_way_through() {
        let key = "ghp_PLANTEDTOKENVALUE0123456789";
        let cfg = ExecConfig {
            secrets: crate::SecretsConfig {
                redact: false,
                ..Default::default()
            },
            ..ExecConfig::default()
        };
        let o = run_cmd(&format!("echo {key}"), cfg).await;
        assert!(o.output.contains(key), "opt-out honoured");
        assert!(o.redacted.is_empty());
    }

    // --- spending a secret (P3) ----------------------------------------------

    /// A table holding one pretend secret, and the handle minted for it.
    fn vault_with(name: &str, value: &str) -> (ExecConfig, String) {
        let mut aliases = crate::Aliases::default();
        let handle = aliases.insert(name, value, "test", true).expect("mint");
        let cfg = ExecConfig {
            aliases: std::sync::Arc::new(aliases),
            ..ExecConfig::default()
        };
        (cfg, handle)
    }

    /// **The end-to-end substitution test.** The shell must receive the real
    /// value where the model wrote a handle.
    ///
    /// Asserted by having the *shell* compare, so the proof never requires the
    /// value to appear in captured output — a test that printed the secret to
    /// check it arrived would be the exact leak this feature exists to prevent.
    #[tokio::test]
    async fn a_handle_reaches_the_shell_as_the_real_value() {
        let (cfg, handle) = vault_with("gh", crate::fakes::GITHUB_TOKEN);
        let o = run_cmd(
            &format!(
                "test \"{handle}\" = {} && echo MATCH",
                crate::fakes::GITHUB_TOKEN
            ),
            cfg,
        )
        .await;
        assert!(o.output.contains("MATCH"), "{o:?}");
        assert_eq!(o.spent, vec!["gh".to_string()]);
    }

    /// The command RECORDED is the one with handles, not the one with values.
    /// This is what keeps a spent secret out of the transcript, the episode log
    /// on disk, and the `info!` line that names every command we run.
    #[tokio::test]
    async fn the_recorded_command_keeps_the_handle_not_the_value() {
        let (cfg, handle) = vault_with("gh", crate::fakes::GITHUB_TOKEN);
        let o = run_cmd(&format!("echo {handle} > /dev/null"), cfg).await;
        assert!(o.command.contains(&handle), "{}", o.command);
        assert!(!o.command.contains("ghp_pretend"), "{}", o.command);
        // And the model is told, so it does not conclude the placeholder ran
        // literally and "fix" a bug that isn't there.
        assert!(
            o.report().contains("secret substituted at execution: gh"),
            "{}",
            o.report()
        );
    }

    /// The return leg: a command that expands a secret and then echoes it gets
    /// the value mapped back to the handle the model already knows — usable, and
    /// never cleartext. A secret spent can be a secret echoed (`secrets.md` §6.5).
    #[tokio::test]
    async fn an_echoed_secret_comes_back_as_its_handle() {
        let (cfg, handle) = vault_with("gh", crate::fakes::GITHUB_TOKEN);
        let o = run_cmd(&format!("echo {handle}"), cfg).await;
        assert!(!o.output.contains("ghp_pretend"), "{:?}", o.output);
        assert!(o.output.contains(&handle), "{:?}", o.output);
    }

    /// **Fail closed.** An unminted handle is the shape of an injection attempt;
    /// expanding it to nothing would turn `rm -rf {{x}}/` into `rm -rf /`,
    /// so the command does not run at all.
    #[tokio::test]
    async fn an_unknown_handle_refuses_to_run() {
        let (cfg, _) = vault_with("gh", crate::fakes::GITHUB_TOKEN);
        let o = run_cmd("echo {{never-minted.0000}}", cfg).await;
        assert!(o.spawn_error.is_some(), "{o:?}");
        assert!(o.is_error());
        assert!(o.output.is_empty(), "nothing ran: {:?}", o.output);
        assert!(o.report().contains("could not run"), "{}", o.report());
    }

    /// A secret the user lent us as a *handle* is never also handed to the child
    /// as an environment variable — that would let any command read it directly
    /// and make the whole mechanism theatre.
    ///
    /// **This test used to be vacuous in its load-bearing half**, and the half
    /// it was vacuous about is the only *isolation*-rung control in the app.
    /// It asserted `echo ${GITHUB_TOKEN:-unset}` printed `unset` — true on any
    /// machine where `GITHUB_TOKEN` was never set, i.e. almost all of them,
    /// whether or not the child inherits the environment. Deleting
    /// `.env_clear()` left all 1,244 tests green (Reviewer E).
    ///
    /// Two fixes, both mechanical:
    ///
    /// - **Plant the thing whose absence is asserted.** An absence test that
    ///   does not create the value is measuring the machine it runs on.
    /// - **Assert the COMPLEMENT.** Naming three credentials tests a list;
    ///   asserting the child's environment is *exactly* the allowlist tests the
    ///   rule, and also catches a future `KEEP` entry that should not be there
    ///   (`[[feedback_mutation_check_guards]]`: a list cannot check that it
    ///   lists everything).
    #[tokio::test]
    async fn a_lent_secret_is_not_in_the_child_environment() {
        // SAFETY: single-threaded within this test's scope of interest; the
        // value is removed before the assertions that could observe it
        // elsewhere. Planting is the point — without it the assertion below is
        // about this machine rather than about `env_clear`.
        unsafe {
            std::env::set_var("SHANNON_TEST_PLANTED_TOKEN", "ghp_planted_do_not_leak");
        }
        let o = run_cmd("env", ExecConfig::default()).await;
        unsafe {
            std::env::remove_var("SHANNON_TEST_PLANTED_TOKEN");
        }

        assert!(
            !o.output.contains("ghp_planted_do_not_leak"),
            "a variable the parent holds must not reach a tool child: {}",
            o.output
        );
        // The complement: every name the child sees is one we chose to pass —
        // the allowlist, or a hygiene default we set. `env` itself adds a few
        // of its own (`_`, `SHLVL`, `PWD`, `OLDPWD`) depending on the shell,
        // so those are allowed by name rather than by loosening the rule.
        let allowed: std::collections::HashSet<&str> = KEEP
            .iter()
            .copied()
            .chain(HYGIENE.iter().map(|(name, _)| *name))
            .chain(["_", "SHLVL", "PWD", "OLDPWD"])
            .collect();
        for name in o.output.lines().filter_map(|l| l.split('=').next()) {
            assert!(
                allowed.contains(name),
                "{name} reached a tool child but is not on the allowlist"
            );
        }
    }

    /// **`[exec] env` reaches a command, over the minimal set** — a `PATH`
    /// set there is the one it runs with, and a name of yours is added; the
    /// rest stays the allowlist (Steven, 10-06).
    #[tokio::test]
    async fn the_configured_env_is_laid_over_the_minimal_set() {
        let cfg = ExecConfig {
            env: vec![
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("SHANNON_TEST_MINE".into(), "mine".into()),
            ],
            ..ExecConfig::default()
        };
        let o = run_cmd("echo \"$PATH|$SHANNON_TEST_MINE\"", cfg).await;
        assert_eq!(o.output.trim(), "/usr/bin:/bin|mine", "{o:?}");
    }

    /// **A withheld name reaches no command of Shannon's own** — one the
    /// minimal set holds (`HOME`), one a hygiene default sets (`PAGER`), and
    /// one `[exec] env` names: `[exec] withhold`, or a lent secret's
    /// variable, wins over all three (Steven, 10-31, S18-Q1).
    #[tokio::test]
    async fn a_withheld_name_reaches_no_command() {
        let cfg = ExecConfig {
            env: vec![("SHANNON_TEST_MINE".into(), "mine".into())],
            withheld: vec!["HOME".into(), "PAGER".into(), "SHANNON_TEST_MINE".into()],
            ..ExecConfig::default()
        };
        let o = run_cmd(
            "echo \"${HOME:-none}|${PAGER:-none}|${SHANNON_TEST_MINE:-none}|$GIT_PAGER\"",
            cfg,
        )
        .await;
        assert_eq!(o.output.trim(), "none|none|none|cat", "{o:?}");
    }

    /// **A hygiene default reaches a command, and `[exec] env` overrides
    /// it** — `git log` under Shannon pages through `cat`, unless the user
    /// said otherwise; the table has one entry per name, whoever set it.
    #[tokio::test]
    async fn a_hygiene_default_is_overridden_by_exec_env() {
        let o = run_cmd(
            "echo \"$GIT_PAGER|$PAGER|$NO_COLOR\"",
            ExecConfig::default(),
        )
        .await;
        assert_eq!(o.output.trim(), "cat|cat|1", "the defaults: {o:?}");
        let cfg = ExecConfig {
            env: vec![("PAGER".into(), "less".into())],
            ..ExecConfig::default()
        };
        let o = run_cmd("echo \"$GIT_PAGER|$PAGER\"", cfg).await;
        assert_eq!(
            o.output.trim(),
            "cat|less",
            "yours wins, the rest stand: {o:?}"
        );

        // The one door: no name twice, the last writer kept.
        let table = child_env(&[("PAGER".to_string(), "less".to_string())], &[]);
        let mut names: Vec<&str> = table.iter().map(|(k, _)| k.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), table.len(), "a name listed twice: {table:?}");
        assert!(table.contains(&("PAGER".to_string(), "less".to_string())));
        assert!(table.contains(&("GIT_PAGER".to_string(), "cat".to_string())));
    }

    /// **The shell is Shannon's, whatever `PATH` a command is given** — a
    /// `Command` given a `PATH` searches it for its program, so
    /// `[exec] env`'s chose the shell (review 29, Reviewer B). The premise
    /// is probed: a bare name under a `PATH` holding nothing is not found.
    #[tokio::test]
    async fn the_shell_is_found_on_shannons_own_path() {
        let bare = tokio::process::Command::new("bash")
            .env_clear()
            .env("PATH", "/nonexistent")
            .args(["-c", "true"])
            .spawn();
        assert!(bare.is_err(), "premise: the child's PATH is searched");
        let cfg = ExecConfig {
            env: vec![("PATH".into(), "/nonexistent".into())],
            ..ExecConfig::default()
        };
        let o = run_cmd("echo \"$PATH\"", cfg).await;
        assert_eq!(o.output.trim(), "/nonexistent", "{o:?}");
        assert!(on_own_path("sh").is_absolute());
        assert_eq!(on_own_path("./sh"), std::path::Path::new("./sh"));

        // As `execvp` finds one: a file not executable, or a directory, of
        // that name is passed over.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (a, b, c) = (
            dir.path().join("a"),
            dir.path().join("b"),
            dir.path().join("c"),
        );
        std::fs::create_dir_all(a.join("tool")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::create_dir_all(&c).unwrap();
        for (d, mode) in [(&b, 0o644), (&c, 0o755)] {
            std::fs::write(d.join("tool"), "").unwrap();
            std::fs::set_permissions(d.join("tool"), std::fs::Permissions::from_mode(mode))
                .unwrap();
        }
        let path = std::env::join_paths([&a, &b, &c]).unwrap();
        let found = found_on(std::path::Path::new("tool"), &path);
        assert_eq!(found, Some(c.join("tool")));
    }

    /// **`$NAME` and `${NAME}` read Shannon's environment; Shannon's own key
    /// is never handed on, as a name or through a value; a `PATH` a file
    /// could run from is said** — and a name no environment holds, or a
    /// value none can (a NUL), is left out, said too. A `${` never closed
    /// ends the expansion: what follows is kept as written.
    #[test]
    fn the_configured_env_expands_refuses_and_says() {
        let mut lookup = |name: &str| (name == "HOME").then(|| "/home/me".to_string());
        assert_eq!(
            expand("$HOME/bin:${HOME}/x", &mut lookup),
            "/home/me/bin:/home/me/x"
        );
        assert_eq!(expand("$UNSET-a", &mut lookup), "-a", "unset: empty");
        assert_eq!(
            expand("cost $ 5 ${", &mut lookup),
            "cost $ 5 ${",
            "a lone `$` is itself"
        );
        assert_eq!(
            expand("a${b $HOME", &mut lookup),
            "a${b $HOME",
            "an unclosed `${{` ends it"
        );

        let key = crate::config::API_KEY_VAR;
        let through = format!("${key}-y");
        let table: std::collections::BTreeMap<String, String> = [
            ("PATH", "/usr/bin:.:bin:"),
            (key, "sk-x"),
            (crate::backend::claude_code::EFFORT_VAR, "high"),
            ("A=B", "x"),
            ("NU\0L", "x"),
            ("NULLED", "a\0b"),
            ("SSH_AUTH_SOCK", "/tmp/agent"),
            ("THROUGH", through.as_str()),
            // A secret's variable, as a name and through another's value
            // (review stop 14, Reviewer E F1).
            ("LENT_TOKEN", "x"),
            ("SPENDS", "$LENT_TOKEN-z"),
            // And one `[exec] withhold` names (Steven, 10-31, S18-Q1).
            ("WITHHELD", "x"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let (env, said) = configured_env(
            &table,
            &["LENT_TOKEN".to_string()],
            &["WITHHELD".to_string()],
        );
        let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            ["PATH", "SPENDS", "SSH_AUTH_SOCK", "THROUGH"],
            "{said:?}"
        );
        assert_eq!(env[1].1, "-z", "a lent value never in a value");
        assert_eq!(env[3].1, "-y", "the key's value never in a value");
        let say = said.join("\n");
        for heard in [
            "PATH holds .",
            "PATH holds bin",
            "an empty entry",
            "is Shannon's own",
            "`A=B` is no variable's name",
            "is no variable's name",
            "`NULLED`'s value holds a NUL",
            &format!("`THROUGH` names `${key}`"),
            "`LENT_TOKEN` is lent as a secret",
            "`SPENDS` names `$LENT_TOKEN`, lent as a secret",
            "`WITHHELD` is in [exec] withhold",
        ] {
            assert!(say.contains(heard), "{heard}: {say}");
        }
        assert_eq!(said.len(), 12, "{said:?}");
    }

    #[test]
    fn truncation_never_splits_a_character() {
        // 4 bytes: 'a' + a 3-byte '€'.
        let buf = "a€".as_bytes();
        assert_eq!(clamp_to_char_boundary(buf, 2), 1, "backs off mid-character");
        assert_eq!(clamp_to_char_boundary(buf, 4), 4);
        assert_eq!(clamp_to_char_boundary(buf, 0), 0);
    }
}

#[cfg(test)]
mod keep_redacted_tests {
    use super::*;

    const KEY: &str = "sk-ant-api03-PLANTEDKEYVALUE0123456789";

    /// A file holding a planted key, `cat`-ed — so the key reaches the record
    /// only through the OUTPUT, never through the command line in its header.
    fn planted(dir: &std::path::Path) -> String {
        let file = dir.join("creds");
        std::fs::write(&file, format!("TOKEN={KEY}\n")).expect("write");
        format!("cat {}", file.display())
    }

    /// **Opted in: the value is kept — owner-only — and the model is told
    /// nothing about it.** Redaction is unchanged: the output and the model's
    /// text are still scrubbed; only the user's copy holds the cleartext.
    #[tokio::test]
    async fn an_opted_in_redaction_keeps_an_owner_only_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let record = tmp.path().join("state/redacted");
        let cfg = ExecConfig {
            keep_redacted_in: Some(record.clone()),
            ..ExecConfig::default()
        };
        let o = run(
            "toolu_01/../x".to_string(),
            &planted(tmp.path()),
            &cfg,
            &Leftovers::default(),
        )
        .await;

        assert!(
            !o.output.contains(KEY) && !o.report().contains(KEY),
            "premise: redacted"
        );
        let path = match &o.kept {
            Some(Ok(p)) => std::path::PathBuf::from(p),
            other => panic!("a copy was asked for and not kept: {other:?}"),
        };
        assert!(
            path.starts_with(&record),
            "written where asked: {}",
            path.display()
        );
        let file_name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            !file_name.contains('/') && !file_name.contains(".."),
            "id sanitised: {file_name}"
        );
        assert!(
            std::fs::read_to_string(&path).unwrap().contains(KEY),
            "the value is kept"
        );
        assert!(
            !o.report().contains(&path.display().to_string()),
            "the model is not told where"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode =
                |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600, "the copy is owner-only");
            assert_eq!(mode(&record), 0o700, "and so is its directory");
        }
    }

    /// Off (the default): nothing is written, and the outcome says so.
    #[tokio::test]
    async fn by_default_nothing_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let o = run(
            "t".to_string(),
            &planted(tmp.path()),
            &ExecConfig::default(),
            &Leftovers::default(),
        )
        .await;
        assert!(!o.redacted.is_empty(), "premise: something was redacted");
        assert_eq!(o.kept, None);
    }

    /// Nothing redacted, nothing kept — even when opted in.
    #[tokio::test]
    async fn clean_output_writes_no_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let record = tmp.path().join("redacted");
        let cfg = ExecConfig {
            keep_redacted_in: Some(record.clone()),
            ..ExecConfig::default()
        };
        let o = run("t".to_string(), "echo hello", &cfg, &Leftovers::default()).await;
        assert_eq!(o.kept, None);
        assert!(!record.exists(), "no directory created for nothing");
    }

    /// **A symlink planted at the directory's name is refused, not followed** —
    /// the copy would otherwise land wherever the link points. The failure is
    /// reported (`Err`), so the notice can say the copy was NOT kept.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlinked_record_directory_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let record = tmp.path().join("redacted");
        std::os::unix::fs::symlink(&elsewhere, &record).unwrap();
        let cfg = ExecConfig {
            keep_redacted_in: Some(record),
            ..ExecConfig::default()
        };
        let o = run(
            "t".to_string(),
            &planted(tmp.path()),
            &cfg,
            &Leftovers::default(),
        )
        .await;
        assert!(
            matches!(o.kept, Some(Err(_))),
            "refused and reported: {:?}",
            o.kept
        );
        assert_eq!(
            std::fs::read_dir(&elsewhere).unwrap().count(),
            0,
            "nothing written through"
        );
    }
}
