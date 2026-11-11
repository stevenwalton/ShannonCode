//! Shannon Code TUI binary.
//!
//! M1 end-state: an output pane on top, a fixed-height prompt pane
//! below it, a status line at the bottom, panes that scroll
//! independently, a streamed stub backend so we can demonstrate
//! scroll-during-output, and a real Anthropic Messages API backend
//! so we can have a conversation with Claude.
//!
//! This file is intentionally thin: it wires logging, resolves which
//! backend to talk to, stands up a tokio runtime, hands control to
//! the terminal guard, and runs the app. The modules it drives live in
//! the library crate (`lib.rs`); the binary is just one consumer of
//! them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use shannon_core::backend::CannedStub;
use shannon_core::config::{CacheConfig, ConfigStack};
use shannon_core::store::project_store_root;
use shannon_core::{Backend, Config, JsonlStore, MemoryStore, Store};
use tracing::{info, warn};

use shannon_tui::app::{App, Opens, with_real_path};
use shannon_tui::bindings;
use shannon_tui::keys::{self, StubReason};
use shannon_tui::logging;
use shannon_tui::terminal::TerminalGuard;

/// **Put a panic in the log before the terminal eats it.**
///
/// `main`'s `Err` arm never sees a panic — unwinding skips it — so the default
/// hook prints to stderr and the process dies. On this program that stderr goes
/// to a terminal still in raw mode and the alternate screen, where it is
/// mangled or invisible, and nothing at all reaches the log file. A crash was
/// the one failure the log could not describe.
///
/// Installed after `init_tracing` and before anything can fail, so the
/// subscriber is alive when the hook fires. `TerminalGuard::drop` deliberately
/// does NOT log during unwind (the appender's worker may already be gone); this
/// runs earlier, while it is not.
///
/// The default hook is called afterwards rather than replaced, so behaviour
/// outside the TUI — `RUST_BACKTRACE`, a test binary — is unchanged.
fn log_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // `location` and the payload are what a backtrace-less report needs;
        // the payload is a `&str` or `String` for a `panic!`, and unprintable
        // otherwise, which is itself worth saying.
        let where_ = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_string());
        let what = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a non-string panic payload".to_string());
        // **Scrubbed, waiting on nothing.** A message can carry what the code
        // held — a probe once caught an API key inside a char-boundary panic —
        // and the log outlives the session. A reading thread's by its job's
        // own rule, the values marked included (review stop 4, E 5); any
        // other by the default policy and no vault: nothing here may take a
        // lock the panicking thread could be holding.
        let what = shannon_tui::app::scrubbed_by_this_threads_reading(&what).unwrap_or_else(|| {
            shannon_core::redact(
                &what,
                &shannon_core::SecretsConfig::default(),
                &shannon_core::Aliases::default(),
            )
            .text
        });
        tracing::error!(location = %where_, panic = %what, "shannon panicked");
        // **Not over the screen, for a panic the app answers for.** A reading
        // thread's is caught and said as a failure, and the app goes on: the
        // default hook's stderr would land on the live alternate screen, and
        // stay — its message unscrubbed, up to 256 bytes of a transcript's
        // text for a char-boundary panic (review stop 4, B 4). The log above
        // has it, scrubbed.
        if shannon_tui::app::on_a_reading_thread() {
            return;
        }
        previous(info);
    }));
}

/// What `shannon` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Invocation {
    Tui,
    /// `shannon --resume [<name|id>]` — open Shannon on that session, as
    /// `:resume` does inside; bare, on the sessions to choose from.
    Resume(Option<String>),
    Setup,
    /// `shannon tutor` — the lessons, the stub answering, in a directory of
    /// their own (`shannon_tui::tutor`).
    Tutor,
    Hook,
    Help,
    Version,
}

const USAGE: &str = "\
usage: shannon [--resume [<name|id>] | setup | tutor | --help | --version]
  shannon                    open Shannon Code
  shannon --resume <name|id> open it on that session (`:resume` inside);
                             bare, choose one in `:graph` (also `-r`)
  shannon setup              choose how it reaches a model (`:setup` inside)
  shannon tutor              learn Shannon, with a stub for a model
(`shannon hook` is Claude Code's, run for each of its tool calls.)
";

/// **The session `--resume` names — never a flag.** A session's name cannot
/// start with `-` (`app::naming`), so `shannon -r --help` took `--help` for
/// a session and opened the TUI to refuse it (review stop 5, E2 #9).
fn resume_of(flag: &str, token: &str) -> Result<Invocation, String> {
    match token.starts_with('-') {
        true => Err(format!(
            "`{flag}` takes a session, and `{token}` is a flag — a session's name never starts \
             with `-`"
        )),
        false => Ok(Invocation::Resume(Some(token.to_string()))),
    }
}

/// **The arguments, read once, or why not.** Anything unknown is refused:
/// it was ignored, so a mistyped `shannon stup` — or `shannon setup` on a
/// binary that predates it — opened the home screen as if it had worked
/// (Steven, 10-04).
///
/// `--resume` (Steven, 10-18, agreeing it comes before testers: "we want
/// `shannon --resume`") is spelled as Claude Code's — `--resume [id]`,
/// `-r` — and `--resume=<id>` reads too.
fn invocation(args: &[String]) -> Result<Invocation, String> {
    match args {
        [] => Ok(Invocation::Tui),
        [flag, rest @ ..] if flag == "--resume" || flag == "-r" => match rest {
            [] => Ok(Invocation::Resume(None)),
            [token] => resume_of(flag, token),
            [_, extra, ..] => Err(format!("`{flag}` takes one session, not `{extra}` too")),
        },
        [flag] if flag.starts_with("--resume=") => {
            match flag.strip_prefix("--resume=").unwrap_or_default() {
                "" => Ok(Invocation::Resume(None)),
                token => resume_of("--resume", token),
            }
        }
        [one] => match one.as_str() {
            "setup" => Ok(Invocation::Setup),
            "tutor" => Ok(Invocation::Tutor),
            "hook" => Ok(Invocation::Hook),
            "-h" | "--help" => Ok(Invocation::Help),
            "-V" | "--version" => Ok(Invocation::Version),
            other => Err(format!("`{other}` is not something shannon does")),
        },
        [first, ..] => Err(format!("`{first}` takes nothing after it")),
    }
}

fn main() -> Result<()> {
    // Times in the locale's form (`tui.time_format = "locale"`), taken before
    // any thread starts: `setlocale` is process-wide (`when::use_the_locale`).
    shannon_core::when::use_the_locale();
    // Lossy, not `args()`: a non-UTF-8 argument panics there, and a panicking
    // hook exits 101, which lets the tool run.
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let invocation = match invocation(&args) {
        Ok(invocation) => invocation,
        Err(why) => {
            eprint!("shannon: {why}\n{USAGE}");
            // 2 as any refusal — and as the hook's, should one ever be called
            // with more than it expects.
            std::process::exit(2);
        }
    };
    match invocation {
        // `shannon hook`: Claude Code's PreToolUse hook, run for every tool
        // call a `claude -p` turn makes. First, before any logging or
        // terminal set-up: it must answer, fast, and fail closed
        // (`shannon_core::hook`).
        Invocation::Hook => std::process::exit(run_hook()),
        Invocation::Help => {
            print!("{USAGE}");
            return Ok(());
        }
        Invocation::Version => {
            println!("shannon {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Invocation::Tui | Invocation::Setup | Invocation::Tutor | Invocation::Resume(_) => {}
    }
    // Both sinks' flush guards — the diary and the audit trail. Held to the end
    // of `main` so neither loses its last lines on exit.
    let _log_guards = logging::init_tracing()?;
    log_panics();
    info!("shannon starting");

    let result = run(&invocation);
    match &result {
        Ok(()) => info!("shannon exited cleanly"),
        Err(e) => tracing::error!(error = ?e, "shannon exited with error"),
    }
    result
}

/// **`shannon hook`**, end to end: the call on stdin, Shannon's answer on
/// stdout or stderr, the exit code returned. Every path but an explicit allow
/// is exit 2, the one Claude Code treats as blocking — a panic included
/// (`hook::fail_closed`), and a failure to WRITE the allow, since `print!`
/// panics on a closed pipe and a panic exits 101, which lets the tool run.
fn run_hook() -> i32 {
    use shannon_core::hook;
    let socket = std::env::var_os(hook::SOCKET_ENV).map(std::path::PathBuf::from);
    let deadline = hook::deadline_from(std::env::var(hook::DEADLINE_ENV).ok().as_deref());
    // **The deadline bounds the whole process**, not only the wait for an
    // answer: `run`'s clock starts once stdin has closed and the socket has
    // connected, so a stdin held open, or a connect that blocks, outlasted it
    // — into Claude Code's own timeout, which lets the tool RUN (review 21,
    // Reviewer B). Exit 2 refuses, whatever is in flight. Written with `let _`:
    // `eprintln!` panics on a closed stderr, and this thread must reach exit.
    std::thread::spawn(move || {
        std::thread::sleep(deadline);
        use std::io::Write as _;
        let _ = writeln!(
            std::io::stderr(),
            "Shannon refused: no decision within the hook's deadline"
        );
        std::process::exit(2);
    });
    hook::run(
        std::io::stdin().lock(),
        socket.as_deref(),
        deadline,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr(),
    )
}

fn run(invocation: &Invocation) -> Result<()> {
    let setup_asked = *invocation == Invocation::Setup;
    // **`shannon tutor` runs in a directory of its own** — entered before
    // anything reads where Shannon started (the trust, the gate's "inside",
    // where a command runs), so a lesson's approved call touches nothing of
    // yours (`tutor.md` T3).
    let tutor = match invocation {
        Invocation::Tutor => {
            let launch = shannon_tui::tutor::Launch::prepare(&std::env::temp_dir())?;
            std::env::set_current_dir(&launch.dir).context("enter the tutor's directory")?;
            Some(launch)
        }
        _ => None,
    };
    // Load config before standing up the runtime: it's cheap, infallible
    // (problems log and fall back to defaults), and the app reads it at
    // construction. It must also come before the backend now, because the
    // cache policy is a CONSTRUCTOR argument rather than a per-request field —
    // every request has to leave an entry or the rolling trail gets holes in
    // it, so there is exactly one place that decides. Everything else that
    // touches state is chosen with it, once ([`chosen`]).
    let Chosen {
        tutor_dir,
        trusted,
        stack: config_stack,
        config,
        notes: mut startup_notes,
        protected,
        claude_sessions,
        store,
        vault,
        scratch,
        knowledge,
        config_file,
    } = chosen(tutor);
    let config = Arc::new(config);
    logging::open_audit_trail(config.exec.audit);
    let (backend, found) = build_backend(config.cache)?;
    let (key_in_use, stub_reason) = match found {
        Ok(key) => (Some(key), None),
        Err(reason) => (None, Some(reason)),
    };
    startup_notes.extend(stub_reason.as_ref().map(keys::stub_note));
    // `shannon setup`, or a first launch: setup opens over the home screen.
    // Never over the tutor, which needs no model to teach.
    let open_setup = tutor_dir.is_none()
        && (setup_asked
            || keys::first_launch(
                stub_reason.as_ref(),
                Config::config_path().is_some_and(|p| p.exists()),
            ));

    // Build the config-merged keymap and install it before the app constructs
    // its per-pane keymaps. An unknown action / bad chord / unknown context
    // warns and is skipped; everything valid still applies.
    let (keymap, warnings) = bindings::Bindings::with_overrides(&config.keymap);
    for w in &warnings {
        warn!("config: {w}");
    }
    // **On screen, not only in the log.** `:help` states this as the rule —
    // "a guard that stops something you asked for says why THERE, never only
    // in a log" — and the launch path was the one place that broke it
    // (Reviewer F).
    startup_notes.extend(warnings.iter().map(|w| format!("keymap: {w}")));
    bindings::set_active(keymap);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;

    let ran = rt.block_on(async {
        let mut term = TerminalGuard::enter(config.tui.mouse)?;
        // The tutor is `b1`, as vimtutor's file is buffer 1: its number is
        // held for it, the page made once launch has read its rules.
        let opens = match tutor_dir {
            Some(_) => Opens::Tutor,
            None => Opens::Session,
        };
        let mut app = App::with_vault(backend, config, store, vault, opens);
        if let Some((key, from)) = key_in_use {
            app.set_key_in_use(key, from);
        }
        app.set_trusted(trusted);
        app.set_claude_sessions(claude_sessions);
        // Claude Code's home, resolved ONCE: its conversations, the gate's
        // `{claude-home}` and the knowledge roots all read this one value.
        app.set_claude_home(shannon_core::claude_import::claude_home());
        // Before the config stack: setting it loads the theme, which may be
        // an imported one (`tui.theme_import`).
        app.set_theme_env(|name| std::env::var(name).ok());
        // The tutor names no file of yours, so `:reload` and `:config edit`
        // have none to read or open, and say why.
        match config_file {
            Ok(yours) => app.set_config_stack(config_stack, yours),
            Err(why) => app.set_config_stack_without_yours(config_stack, why),
        }
        app.set_scratch(scratch);
        // Before anything can run: the gate is rebuilt with them here, and by
        // `rebuild_rules` on every later change of config.
        app.protect_paths(protected);
        // Memory and skills, read from where Shannon started and the roots
        // chosen — after the gate knows Shannon's own paths, which judge
        // every file a location names.
        let (config_dir, home) = knowledge;
        startup_notes.extend(app.launch_knowledge(config_dir, home));
        startup_notes.extend(app.env_notes());
        startup_notes.extend(app.start_notes());
        startup_notes.extend(app.glyph_notes());
        app.report_startup(&startup_notes);
        // `--resume`: what `:resume` does, before setup or the trust question
        // opens over it — so a first launch does not drop it unsaid; a name
        // that matches nothing is refused on screen, as `:resume` refuses.
        if let Invocation::Resume(token) = invocation {
            app.resume_command(token.as_deref().unwrap_or(""));
        }
        // Setup first when it opens: it decides the backend, and asks about
        // this directory itself when that is Claude Code. Without it, the
        // trust screen — or what this directory holds unread. The tutor's
        // directory is its own, and asks only if a lesson sends to Claude
        // Code (`ask_trust_before_sending`).
        if tutor_dir.is_some() {
            app.open_tutor();
        } else if open_setup {
            app.open_setup();
        } else {
            app.ask_trust_up_front();
        }
        app.run(term.terminal()).await
    });
    // Dropping the runtime drops every task — which ends every command's
    // process group, held or running — and then WAITS for every blocking
    // thread, each `waitid` on a shell among them. A shell that cannot die
    // (stuck in the kernel) would hang the exit forever; bounded instead
    // (Reviewer A, round 17). No test reaches `main`.
    rt.shutdown_timeout(std::time::Duration::from_secs(2));
    ran
}

/// Resolve where conversation history is persisted and open a [`JsonlStore`]
/// there: `$XDG_STATE_HOME/shannon/projects/<cwd-hash>/` (per the XDG state spec,
/// falling back to `$HOME/.local/state`), scoped to the launch directory so each
/// project keeps its own history. Best-effort and never fatal — if the state dir
/// or cwd can't be resolved, or the store won't open, the app runs with
/// ephemeral in-memory history (a warning is logged). Key values are never
/// touched here; this is the conversation log, not credentials.
/// Where this project's conversation history lives, if it can be resolved.
///
/// Under `Config::state_dir`, the one resolver `register_own_state` protects
/// as a whole: every project's store, not only this one's. The two must not
/// compute it separately — a store the agent can read is the whole finding,
/// and two answers agreeing by coincidence is how the `SHANNON_API_KEY_FILE`
/// registration came to be about a different file than the one being read.
fn store_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    Some(project_store_root(&Config::state_dir()?, &cwd))
}

fn build_store() -> (Box<dyn Store>, Option<String>) {
    if let Some(root) = store_root() {
        match JsonlStore::new(&root) {
            Ok(store) => {
                info!(root = %root.display(), "persisting conversation history");
                return (Box::new(store), None);
            }
            Err(e) => {
                warn!(root = %root.display(), error = %e, "could not open history store; running ephemeral");
                return (
                    Box::new(MemoryStore::new()),
                    Some(EPHEMERAL_HISTORY.to_string()),
                );
            }
        }
    }
    warn!("no XDG_STATE_HOME/HOME or working dir; running with ephemeral history");
    (
        Box::new(MemoryStore::new()),
        Some(EPHEMERAL_HISTORY.to_string()),
    )
}

/// **What a launch touches state with** ([`chosen`]).
struct Chosen {
    /// The tutor's own directory, when this is `shannon tutor`.
    tutor_dir: Option<PathBuf>,
    trusted: shannon_tui::trust::Trusted,
    stack: ConfigStack,
    config: Config,
    /// What the launch says on screen, so far.
    notes: Vec<String>,
    /// Shannon's own paths, guarded in every launch ([`app_config`]).
    protected: Vec<String>,
    claude_sessions: shannon_tui::claude_sessions::ClaudeSessions,
    store: Box<dyn Store>,
    vault: Arc<dyn shannon_core::Vault>,
    scratch: Vec<String>,
    /// Where memory and skills are read from besides the start: `(config
    /// directory, home)` (`App::launch_knowledge`).
    knowledge: (Option<PathBuf>, Option<PathBuf>),
    /// The file `:reload` and `:config edit` read and open — or why there is
    /// none of yours.
    config_file: Result<Option<PathBuf>, &'static str>,
}

/// **Every choice of a launch that touches state, made once** — the tutor's
/// from `tutor::Launch`, taken whole (every field named, so one added there
/// is an unused binding until it is used here), an ordinary launch's read
/// from your files ([`ordinary`]). One value, so a step `run` adds is given
/// a value for each launch rather than an arm of its own. There were seven
/// `match tutor` arms, and the steps nobody had decided for the tutor —
/// memory and skills read from your config directory and home, your Claude
/// Code sessions file, the scratch directory — made a directory in your
/// config tree at every tutor run (review stop 16, D 1).
fn chosen(tutor: Option<shannon_tui::tutor::Launch>) -> Chosen {
    let Some(launch) = tutor else {
        return ordinary();
    };
    let (knowledge, scratch) = (launch.knowledge(), launch.scratch());
    let shannon_tui::tutor::Launch {
        dir,
        stack,
        config,
        store,
        vault,
        trusted,
        claude_sessions,
    } = launch;
    let (stack, config, mut notes, protected) = app_config(None, None, Some((stack, config)));
    notes.push(format!(
        "the tutor — its chats, secrets and trust last this run; its files are in {}",
        dir.display()
    ));
    Chosen {
        tutor_dir: Some(dir),
        trusted,
        stack,
        config,
        notes,
        protected,
        claude_sessions,
        store,
        vault,
        scratch,
        knowledge,
        config_file: Err(shannon_tui::tutor::READS_NO_CONFIG),
    }
}

/// An ordinary launch's choices ([`chosen`]): your trust file, the config
/// (with a trusted project's own file), your Claude Code sessions, the store
/// under your state directory, the scratch directory, the vault, and memory
/// and skills read from your config directory and home.
fn ordinary() -> Chosen {
    // The directories Shannon trusts (`trust`) — read first, since a trusted
    // start's own `.shannon/config.toml` is a layer of the config (6d). One
    // that cannot be read trusts nothing, and says so.
    let (trusted, trust_note) =
        shannon_tui::trust::Trusted::load(shannon_tui::trust::default_path());
    let start = std::env::current_dir().ok();
    let project = start.as_deref().and_then(|s| trusted.project_file(s));
    let told = project.as_deref().and_then(|p| trusted.told(p));
    let (stack, config, mut notes, protected) = app_config(project.as_deref(), told, None);
    notes.extend(trust_note);
    // The Claude Code sessions Shannon started: resumed across a restart,
    // and kept out of `:graph`'s list of Claude Code's own.
    let (claude_sessions, sessions_note) = shannon_tui::claude_sessions::ClaudeSessions::load(
        shannon_tui::claude_sessions::default_path(),
    );
    notes.extend(sessions_note);
    let (store, store_note) = build_store();
    notes.extend(store_note);
    // The scratch directory, vouched for once (`scratch::prepare`): an
    // approved place only if it is a directory, this user's, closed to others
    // — and if it is not, said, and nothing is approved there.
    let scratch_path = config
        .permissions
        .scratch
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(shannon_core::scratch::default_path);
    let scratch = match shannon_core::scratch::prepare(&scratch_path) {
        Ok(spellings) => spellings,
        Err(why) => {
            notes.push(format!("no scratch directory — {why}"));
            Vec::new()
        }
    };
    // Where marked secrets wait between runs: Shannon's own store, one
    // whatever `[secrets] password_protect` says; with no state directory,
    // none — said, never silent.
    let (vault, why) = shannon_tui::app::os_vault();
    notes.extend(why);
    let knowledge = (
        Config::config_path().and_then(|p| p.parent().map(Path::to_path_buf)),
        shannon_core::secrets::home_dir().map(PathBuf::from),
    );
    Chosen {
        tutor_dir: None,
        trusted,
        stack,
        config,
        notes,
        protected,
        claude_sessions,
        store,
        vault,
        scratch,
        knowledge,
        config_file: Ok(Config::config_path()),
    }
}

/// What the user is told when history is not reaching disk.
///
/// **On screen, not only in the log.** `:help` states the rule — "a guard that
/// stops something you asked for says why THERE, never only in a log" — and
/// this is the failure whose whole cost is discovered later: the session reads
/// exactly like a saved one until you relaunch and it is gone
/// (`[[feedback_no_silent_failure]]`).
const EPHEMERAL_HISTORY: &str =
    "history is NOT being saved — this session lives in memory only (see the log for why)";

/// The config this process runs on — **exactly what is on disk**, and the
/// starting effort `CLAUDE_CODE_EFFORT_LEVEL` sets (`start_effort`, a `:set`
/// layer) — plus, SEPARATELY, the paths only the running process can know to
/// protect.
///
/// Extracted so the registration is reachable from a test. It lived inline in
/// [`run`] and was lost in the reconstruction without a single test going red,
/// because nothing imports a binary's `run` (Reviewer E).
///
/// **The registrations are not written into the config**, and must not be:
/// they used to be pushed into `config.secrets.paths`, and `:config edit`
/// replaces the config with the file as parsed, which never held them — so
/// every edit silently un-protected Shannon's own state for the rest of the run
/// (`SECURITY-2026-09-24-config-edit-drops-own-state.md`).
///
/// The stack comes back with the config: it is what `:source`, `:reload` and
/// `:unsource` add to and take from, so the running config can always be
/// re-derived from the files that made it.
///
/// `told`: what the project's file was last said to change about safety
/// (`Trusted::told`), so its note is not said at every launch.
///
/// `tutor`: the stack `shannon tutor` runs on (`tutor::Launch`), none of
/// your files in it — vimtutor's `-u NONE`, and no `:reload` can lay a model
/// of yours back over its stub.
fn app_config(
    project: Option<&Path>,
    told: Option<&str>,
    tutor: Option<(ConfigStack, Config)>,
) -> (ConfigStack, Config, Vec<String>, Vec<String>) {
    let (mut stack, mut config, mut notes) = match tutor {
        Some((stack, config)) => (stack, config, Vec::new()),
        None => ConfigStack::load_reporting(project, told),
    };
    let effort_var = std::env::var_os(shannon_core::backend::claude_code::EFFORT_VAR);
    notes.extend(stack.start_effort(&mut config, effort_var.as_deref()));
    let cwd = std::env::current_dir().ok();
    let mut protected = keys::protected_key_files(&keys::key_files_for(), cwd.as_deref());
    register_own_state(&mut protected, &mut notes);
    (stack, config, notes, protected)
}

/// Mark every path **this process writes** sensitive, so the agent cannot read
/// Shannon's own state unattended.
///
/// The shipped seed named `~/.claude` — a competitor's credential directory —
/// and none of ours. Verified before this landed: `cat ~/.config/shannon/
/// config.toml`, `cat ~/.local/state/shannon/log/shannon.log` and
/// `cat ~/.local/state/shannon/projects/<hash>/episodes/*.jsonl` were all
/// `Allow`, **unattended** (Reviewer E). Each is a real disclosure:
///
/// - `config.toml` may hold cleartext credentials — `[[alias]] value = "…"` is
///   an accepted spelling.
/// - the log records every command every session ran, plus API error bodies.
/// - the episode store is **per project**, so a session in one project reading
///   it is reading another project's conversations. The 0700/0600 modes do not
///   help: the tool child runs as the same user.
///
/// A model asked "what did we do last session" has no memory of it and is told
/// to look at the machine rather than recall, so this is the shortest path, not
/// an exotic one.
///
/// **Registered from each resolver, not written as patterns.** A pattern cannot
/// cover a path the user chose (the `SHANNON_API_KEY_FILE` lesson), and a list
/// of three rows is a proxy for "places this program keeps secrets" that goes
/// stale the day a fourth surface is added
/// (`[[feedback_enumeration_means_unmodeled]]`).
fn register_own_state(protected: &mut Vec<String>, notes: &mut Vec<String>) {
    let mut add = |p: Option<PathBuf>| {
        for p in p.map(|p| with_real_path(&p)).unwrap_or_default() {
            protected.push(p.display().to_string());
        }
    };
    add(Config::config_path());
    // The user's program models beside it: a model can loosen the gate as
    // the config can (`rm.toml` saying `effect = "read"`), so a write there
    // asks wherever the config lives — the shipped `~/.config/shannon`
    // guard is only the default place (P6b).
    add(Config::config_path().and_then(|p| {
        p.parent()
            .map(|dir| dir.join(shannon_core::permission::model::USER_DIR))
    }));
    // The API key `shannon setup` saves beside it: Shannon writes it, so it
    // is Shannon's state, whether or not a named key file is read first.
    add(Config::api_key_path());
    // The machine-wide config, when one is installed: it is read into this
    // process at launch like yours, from the same resolver.
    add(Config::system_config_path());
    match crate::logging::log_dir() {
        Ok(dir) => add(Some(dir)),
        // Not fatal — logging itself already falls back — but the user should
        // know a credential surface went unregistered rather than find out
        // from a transcript.
        Err(e) => notes.push(format!("could not protect the log directory: {e}")),
    }
    // The WHOLE state directory, not this project's store: it registered only
    // `store_root()`, so a session here could read every OTHER project's
    // conversations, which is the disclosure the list above names.
    add(Config::state_dir());
    // Unredacted output (`secrets.keep_redacted`). Registered whether or not
    // the setting is on: files from an earlier session outlive the setting,
    // and it can be switched on mid-session.
    add(Config::redacted_record_dir());
    // Secrets you marked, kept without a password (`[secrets]
    // password_protect` off): under the state directory already, named so
    // the protection does not rest on where the resolver happens to put it.
    add(Config::kept_secrets_dir());
}

/// **Which backend to talk to**: the API, from the key `keys::find_key` finds
/// (its rule and its tests live there), or the canned stub with why. The app
/// stays runnable without a key, so the TUI can be exercised offline. Where
/// the key came from is logged; the key never is. The key goes on to the app,
/// masked, for setup to say and to save (`App::set_key_in_use`).
fn build_backend(cache: CacheConfig) -> Result<(Arc<dyn Backend>, Result<KeyInUse, StubReason>)> {
    let found = keys::find_key(
        std::env::var(keys::API_KEY_VAR).ok(),
        &keys::key_files_for(),
        |path| std::fs::read_to_string(path),
    );
    match found {
        Ok((key, from)) => {
            info!(from = %from, "using an API key");
            let backend = keys::api_backend(key.clone(), cache)?;
            Ok((backend, Ok((key, from))))
        }
        Err(reason) => {
            match &reason {
                StubReason::NoKey => info!("no API key found; answering from the canned stub"),
                other => warn!(reason = ?other, "the API key could not be used; canned stub"),
            }
            Ok((Arc::new(CannedStub::new()), Err(reason)))
        }
    }
}

/// The key launch found, and where it came from.
type KeyInUse = (String, keys::KeyFrom);

#[cfg(test)]
mod tests {
    use super::*;

    /// **What shannon does, and a refusal for anything else** — an unknown
    /// argument was ignored, so a typo opened the home screen as if it had
    /// worked (Steven, 10-04).
    #[test]
    fn an_unknown_argument_is_refused_not_ignored() {
        let read =
            |args: &[&str]| invocation(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>());
        assert_eq!(read(&[]), Ok(Invocation::Tui));
        assert_eq!(read(&["setup"]), Ok(Invocation::Setup));
        assert_eq!(read(&["tutor"]), Ok(Invocation::Tutor));
        assert!(read(&["tutor", "now"]).is_err());
        assert_eq!(read(&["hook"]), Ok(Invocation::Hook));
        assert_eq!(read(&["--help"]), Ok(Invocation::Help));
        assert_eq!(read(&["-V"]), Ok(Invocation::Version));
        assert!(read(&["stup"]).is_err_and(|why| why.contains("stup")));
        assert!(read(&["setup", "now"]).is_err());
        // `--resume`, as Claude Code spells it (Steven, 10-18).
        assert_eq!(read(&["--resume"]), Ok(Invocation::Resume(None)));
        assert_eq!(read(&["--resume="]), Ok(Invocation::Resume(None)));
        for args in [
            &["--resume", "ab12"][..],
            &["-r", "ab12"],
            &["--resume=ab12"],
        ] {
            assert_eq!(read(args), Ok(Invocation::Resume(Some("ab12".into()))));
        }
        assert!(read(&["--resume", "a", "b"]).is_err_and(|why| why.contains("`b`")));
        // A flag is never a session (E2 #9): a name cannot start with `-`.
        for args in [&["-r", "--help"][..], &["--resume", "-x"], &["--resume=-x"]] {
            assert!(
                read(args).is_err_and(|why| why.contains("is a flag")),
                "{args:?}"
            );
        }
        assert!(read(&["--resum"]).is_err(), "no guessing at a near miss");
        assert!(
            read(&["hook", "x"]).is_err(),
            "and exits 2: a refusal, to Claude Code"
        );
    }

    /// Everything logged while `f` runs. One helper, because two tests here
    /// each want it and a second copy is a second thing to keep in step.
    fn capture(f: impl FnOnce()) -> String {
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("sink").extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Sink {
            type Writer = Sink;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(Sink(Arc::clone(&buf)))
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        String::from_utf8_lossy(&buf.lock().expect("sink").clone()).into_owned()
    }

    /// **The panic hook is the process's, one at a time.** The two tests that
    /// install it take the hook, set their own and put the old one back; run
    /// at once, one's restore replaced the other's hook mid-panic, and that
    /// one's log came back empty — a red suite one run in many (10-21).
    static PANIC_HOOK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// **A panic reaches the log**, which is the one failure it could not
    /// describe before: unwinding skips `main`'s `Err` arm, and the default
    /// hook's stderr goes to a terminal in raw mode.
    ///
    /// Installs the real hook, panics on a scratch thread so the harness is
    /// unaffected, and reads back what the subscriber saw. The default hook is
    /// silenced for the duration or the suite's output fills with the
    /// deliberate panic.
    #[test]
    fn a_panic_is_written_to_the_log() {
        let _hook = PANIC_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let logged = capture(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            log_panics();
            // On this thread, so the scoped subscriber is the one the hook
            // writes to.
            let _ = std::panic::catch_unwind(|| panic!("a deliberate test panic"));
            let _ = std::panic::take_hook();
            std::panic::set_hook(previous);
        });
        assert!(logged.contains("shannon panicked"), "{logged:?}");
        assert!(
            logged.contains("a deliberate test panic"),
            "the payload is what makes it diagnosable: {logged:?}"
        );
        assert!(
            logged.contains("main.rs:"),
            "the location is the other half: {logged:?}"
        );
    }

    /// **A panic's message is scrubbed before it is logged.** A message can
    /// carry what the code was holding — a probe once caught a whole API key
    /// inside a char-boundary panic — and the log is a file on disk, read
    /// after the session. Never scrubbed until review 20's sweep.
    #[test]
    fn a_panic_message_is_scrubbed_before_it_is_logged() {
        const KEY: &str = "sk-ant-api03-abcdefghijklmnopqrstuvw";
        let _hook = PANIC_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let logged = capture(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            log_panics();
            let _ = std::panic::catch_unwind(|| panic!("slicing ANTHROPIC_API_KEY={KEY} failed"));
            let _ = std::panic::take_hook();
            std::panic::set_hook(previous);
        });
        assert!(logged.contains("shannon panicked"), "premise: {logged:?}");
        assert!(!logged.contains(KEY), "{logged:?}");
        assert!(
            logged.contains("slicing"),
            "the rest still diagnoses: {logged:?}"
        );
    }

    /// The ephemeral-history note names the consequence, not the cause.
    ///
    /// A user who reads "could not open the store" does not know their work is
    /// about to vanish; one who reads "history is NOT being saved" does.
    #[test]
    fn the_ephemeral_history_note_names_the_consequence() {
        assert!(
            EPHEMERAL_HISTORY.contains("NOT being saved"),
            "{EPHEMERAL_HISTORY}"
        );
    }
    /// Does the gate let `cat` read `path` unasked, given `protected` as
    /// launch hands it over (`App::protect_paths` → `with_own_state`)? Started
    /// at `/`, so every read is inside and allowed by default: only a guard
    /// asks. The gate's own door — these tests asked `secrets::sensitive_path`,
    /// a glob matcher the gate never called (since deleted, Q1), where the
    /// gate reads own state as literal paths (review stop 12, T6 2). The
    /// lib's `keys` tests hold its twin; a binary's tests cannot reach a
    /// library's.
    fn read_unasked(protected: &[String], path: &str) -> bool {
        let rules = shannon_core::permission::RuleSet::defaults()
            .with_start("/")
            .with_own_state(protected.to_vec());
        rules.decide_line(&format!("cat '{path}'")).decision
            == shannon_core::permission::Decision::Allow
    }

    /// **Shannon's own on-disk state is refused, and by the same resolver that
    /// creates it.**
    ///
    /// The shipped seed protected `~/.claude` and none of ours: `config.toml`
    /// (which may hold a cleartext `[[alias]] value`), the log (every command
    /// every session ran) and the per-project episode store (another project's
    /// conversations) were all `Allow`, unattended (Reviewer E).
    ///
    /// Drives the **producer's own output through the consumer** rather than
    /// asserting on a string this test wrote: a resolver that starts returning
    /// a different directory must fail here
    /// (`[[feedback_assert_the_subject]]`). Asked of the gate, so a
    /// registration that lands somewhere the gate cannot see also fails.
    #[test]
    fn the_running_config_refuses_its_own_state_directories() {
        // The registrations are their own list now, never the config's; the
        // app adds them to the gate (`App::protect_paths`, tested there against
        // a config edit). Here: the list the resolvers produce, through the
        // gate.
        let (mut protected, mut notes) = (Vec::new(), Vec::new());
        register_own_state(&mut protected, &mut notes);

        // Whatever the resolvers actually returned — not a path spelled here.
        for want in [
            Config::config_path(),
            // A program model: it can loosen the gate as the config can.
            Config::config_path().and_then(|p| {
                p.parent().map(|d| {
                    d.join(shannon_core::permission::model::USER_DIR)
                        .join("rm.toml")
                })
            }),
            // The key `shannon setup` saves — no seed covers `api_key`, so
            // this registration is the whole of its protection (review 21,
            // Reviewer C: the key-file loop that also lists it is untested).
            Config::api_key_path(),
            store_root(),
            // ANOTHER project's conversations: only this project's store was
            // registered, which is the cross-project read this exists to stop.
            Config::state_dir().map(|d| d.join("projects/0123456789abcdef/episodes/e.jsonl")),
            Config::redacted_record_dir().map(|d| d.join("1-toolu_x.txt")),
        ] {
            let Some(p) = want else {
                continue; // no HOME in this environment; nothing to protect
            };
            let shown = p.display().to_string();
            assert!(
                read_unasked(&[], &shown),
                "premise: only the guard asks of {shown}"
            );
            assert!(
                !read_unasked(&protected, &shown),
                "Shannon writes `{shown}` and must refuse to read it unattended"
            );
        }
        // Premise: this is not vacuous — an ordinary path must NOT match, or
        // the assertion above would pass against a list that matched
        // everything.
        assert!(
            read_unasked(&protected, "/tmp/ordinary.txt"),
            "premise — the check must still discriminate"
        );
    }

    /// **Own state is registered where it really is, too** (review
    /// 2026-10-11, E2): through a linked directory, the link's name and the
    /// target's — for a file written and one not written yet.
    #[cfg(unix)]
    #[test]
    fn own_state_is_registered_where_it_really_is() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles/shannon");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("config.toml"), "").unwrap();
        let link = dir.path().join("config-shannon");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let real = std::fs::canonicalize(&real).unwrap();
        for (name, written) in [("config.toml", true), ("programs", false)] {
            let got = with_real_path(&link.join(name));
            assert_eq!(got.len(), 2, "{name}: {got:?}");
            assert_eq!(got[1], real.join(name), "{name}: written {written}");
        }
        let plain = std::fs::canonicalize(dir.path()).unwrap().join("x");
        assert_eq!(
            with_real_path(&plain),
            vec![plain.clone()],
            "no link, one path"
        );
    }
}
