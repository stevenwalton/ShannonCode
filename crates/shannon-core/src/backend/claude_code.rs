//! **Claude Code as a provider** — a `claude -p` turn, read as Shannon events.
//!
//! `docs/DesignDocs/claude-code-backend.md`. A `claude -p` turn is a whole
//! agent turn: Claude Code runs its own tool loop, under Shannon's gate (the
//! PreToolUse hook, `crate::hook`), and streams a record of it. This module
//! reads that record — `--output-format stream-json --verbose
//! --include-partial-messages`, one JSON object a line — into [`Event`]s.
//!
//! Two halves:
//! - [`read_line`] — pure: a line in, events out. The shapes are the ones
//!   measured by the switch probe (10-03): `system/init` carries the session
//!   id; `stream_event` carries the text as it streams (`content_block_delta`
//!   / `text_delta`); each `assistant` line carries ONE content block (a
//!   `thinking` block, a `tool_use`, a `text`); each `user` line carries the
//!   `tool_result`s Claude Code got back; `result` closes the turn with its
//!   usage. A line whose `parent_tool_use_id` is set belongs to a subagent,
//!   under that call.
//! - [`ClaudeCode`] — the [`Backend`]: one `claude -p` per turn, its hook
//!   answered through a unix socket the turn opens FIRST (no gate, no turn),
//!   its events sent on as [`StreamMsg`]s, its tool questions as
//!   [`StreamMsg::ToolAsk`] for the gate to answer through
//!   [`Backend::answer_tool`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tokio::task::JoinHandle;

use super::{Backend, ProviderEnv, SendRequest};
use crate::config::WebPolicy;
use crate::hook::{self, Reply};
use crate::stream::{AskId, StopReason, ToolQuestion, Usage};
use crate::{BackendError, Block, Message, Role, StreamMsg};

/// What one line of a `claude -p` turn means to Shannon.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The session this turn ran in — kept per episode, for `--resume`.
    Session(String),
    /// The MCP servers it started, from the same `init` line.
    Servers(Vec<crate::McpServer>),
    /// It compacted its own context (`system/compact_boundary`): unasked or
    /// not, and the tokens it held before.
    Compacted { auto: bool, before: u32 },
    /// Reply text as it streams. Only the top-level agent's: a subagent's
    /// text is its own conversation, recorded through its tool calls.
    Text(String),
    /// Thinking text as it streams, for the screen — top-level only, as
    /// [`Event::Text`] is.
    Thinking(String),
    /// **A thinking block, whole** (`thinking` or `redacted_thinking`, from
    /// the `assistant` line that carries it) — kept with the turn and shown;
    /// never sent back, since Claude Code keeps its own history.
    Thought(crate::Block),
    /// **A request began** (`message_start`), with its prompt's usage. Claude
    /// Code makes one per tool round, so one turn is many requests — and the
    /// last one's prompt is what the next turn re-sends. Top-level only, as
    /// a subagent's requests fill a window of their own.
    Request(Usage),
    /// The request's later reading (`message_delta`): its output so far.
    RequestUsage(Usage),
    /// **A top-level tool call's id, as its block begins**
    /// (`content_block_start`) — small and written before the input, which
    /// can make the call's `assistant` line far bigger than a pipe holds.
    /// It names the call to the gate ([`Event::names_a_call`]) and records
    /// nothing: the record waits for the whole call ([`Event::ToolCalled`]).
    ToolNamed(String),
    /// A tool call Claude Code made — already gated by the hook, and RUN BY
    /// CLAUDE CODE, never by Shannon: a record, not a request.
    ToolCalled {
        id: String,
        name: String,
        input: Value,
        /// The subagent call this one ran under, if any.
        parent: Option<String>,
    },
    /// What a tool call returned (or why it did not run).
    ToolReturned {
        id: String,
        output: String,
        is_error: bool,
        parent: Option<String>,
    },
    /// The turn is over: why it stopped, what it used, and whether it failed.
    Finished {
        stop: StopReason,
        usage: Usage,
        /// `result.is_error` — a turn Claude Code itself reports as failed.
        failed: bool,
        /// `result.result`: the closing text, or the failure's message.
        summary: String,
    },
}

impl Event {
    /// **The call this event names to the gate**, if it names one — a call
    /// whole ([`Event::ToolCalled`], a subagent's too) or as it begins
    /// ([`Event::ToolNamed`]). The gate answers only a question about a call
    /// the turn's own output named (`question`).
    pub fn names_a_call(&self) -> Option<&str> {
        match self {
            Event::ToolCalled { id, .. } | Event::ToolNamed(id) => Some(id),
            _ => None,
        }
    }
}

/// **One stream-json line, as the events it carries** — none for a line that
/// means nothing here (thinking, signatures, status, rate limits, the rest of
/// the message framing). An error only for a line that is not a JSON object at all: a
/// shape we do not know is skipped, not refused, since Claude Code adds event
/// types between versions and a turn must not die of one.
pub fn read_line(line: &str) -> Result<Vec<Event>, String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(Vec::new());
    }
    let value: Value =
        serde_json::from_str(line).map_err(|e| format!("not a stream-json line ({e})"))?;
    if !value.is_object() {
        return Err("not a stream-json object".into());
    }
    let parent = value["parent_tool_use_id"].as_str().map(str::to_string);
    let events = match value["type"].as_str() {
        Some("system") if value["subtype"] == "init" => {
            let servers = value["mcp_servers"].as_array().map(|list| {
                list.iter()
                    .filter_map(|s| {
                        Some(crate::McpServer {
                            name: s["name"].as_str()?.to_string(),
                            status: s["status"].as_str().unwrap_or("unknown").to_string(),
                        })
                    })
                    .collect()
            });
            value["session_id"]
                .as_str()
                .map(|id| Event::Session(id.to_string()))
                .into_iter()
                .chain(servers.map(Event::Servers))
                .collect()
        }
        // `compact_metadata` as the Agent SDK documents its
        // `compact_boundary` message.
        Some("system") if value["subtype"] == "compact_boundary" => {
            let meta = &value["compact_metadata"];
            vec![Event::Compacted {
                auto: meta["trigger"] != "manual",
                before: meta["pre_tokens"]
                    .as_u64()
                    .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX)),
            }]
        }
        Some("stream_event") if parent.is_none() => {
            let event = &value["event"];
            match (event["type"].as_str(), event["delta"]["type"].as_str()) {
                (Some("content_block_delta"), Some("text_delta")) => event["delta"]["text"]
                    .as_str()
                    .map(|t| Event::Text(t.to_string()))
                    .into_iter()
                    .collect(),
                // Empty under Claude Code's default (`omitted`): nothing shown.
                (Some("content_block_delta"), Some("thinking_delta")) => event["delta"]["thinking"]
                    .as_str()
                    .filter(|t| !t.is_empty())
                    .map(|t| Event::Thinking(t.to_string()))
                    .into_iter()
                    .collect(),
                // Only with a usage object: a frame without one says nothing
                // about cost, and must not turn the `result`'s fallback off.
                (Some("message_start"), _) if event["message"]["usage"].is_object() => {
                    vec![Event::Request(usage_of(&event["message"]["usage"]))]
                }
                (Some("message_delta"), _) if event["usage"].is_object() => {
                    vec![Event::RequestUsage(usage_of(&event["usage"]))]
                }
                (Some("content_block_start"), _)
                    if event["content_block"]["type"] == "tool_use" =>
                {
                    let id = event["content_block"]["id"].as_str();
                    id.map(|id| Event::ToolNamed(id.to_string()))
                        .into_iter()
                        .collect()
                }
                _ => Vec::new(),
            }
        }
        Some("assistant") => blocks(&value)
            .filter_map(|b| match b["type"].as_str()? {
                "tool_use" => Some(Event::ToolCalled {
                    id: b["id"].as_str()?.to_string(),
                    name: b["name"].as_str()?.to_string(),
                    input: b["input"].clone(),
                    parent: parent.clone(),
                }),
                // The top-level agent's, as its text is: a subagent's
                // thinking is its own conversation's.
                "thinking" | "redacted_thinking" if parent.is_none() => {
                    serde_json::from_value(b.clone()).ok().map(Event::Thought)
                }
                _ => None,
            })
            .collect(),
        Some("user") => blocks(&value)
            .filter(|b| b["type"] == "tool_result")
            .filter_map(|b| {
                Some(Event::ToolReturned {
                    id: b["tool_use_id"].as_str()?.to_string(),
                    output: result_text(&b["content"]),
                    is_error: b["is_error"].as_bool().unwrap_or(false),
                    parent: parent.clone(),
                })
            })
            .collect(),
        Some("result") => vec![Event::Finished {
            stop: value["stop_reason"]
                .as_str()
                .map_or(StopReason::EndTurn, StopReason::from_wire),
            usage: usage_of(&value["usage"]),
            failed: value["is_error"].as_bool().unwrap_or(false) || value["subtype"] != "success",
            summary: value["result"].as_str().unwrap_or_default().to_string(),
        }],
        _ => Vec::new(),
    };
    Ok(events)
}

/// **A usage object, field by field** — the `result` line's, a
/// `message_start`'s and a `message_delta`'s. Read leniently because the
/// three carry different subsets: a delta names only `output_tokens`, and a
/// strict parse of it (two required fields) fell back to all zeros. A count
/// missing or not a number reads as 0, which `Usage::merge` makes harmless.
fn usage_of(usage: &Value) -> Usage {
    let count = |key: &str| {
        usage[key]
            .as_u64()
            .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
    };
    Usage {
        input_tokens: count("input_tokens"),
        output_tokens: count("output_tokens"),
        cache_creation_input_tokens: count("cache_creation_input_tokens"),
        cache_read_input_tokens: count("cache_read_input_tokens"),
    }
}

/// The content blocks of an `assistant` or `user` line.
fn blocks(value: &Value) -> impl Iterator<Item = &Value> {
    value["message"]["content"].as_array().into_iter().flatten()
}

/// A `tool_result`'s content as text: a string, or the text of its blocks
/// (an image is said, not dropped — the record must not read as empty).
fn result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|part| match part["type"].as_str() {
                Some("text") => part["text"].as_str().unwrap_or_default().to_string(),
                Some(other) => format!("[{other}]"),
                None => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

// ---- The backend -----------------------------------------------------------

/// A model id that names Claude Code as its provider — `claude-code/sonnet`
/// (Steven, 10-03). `App::backend_for` routes on it, per session, as it routes
/// `stub`; what follows the prefix is `claude --model`'s own name.
pub const MODEL_PREFIX: &str = "claude-code/";

/// Whether `model` names Claude Code as its provider.
pub fn is_claude_code(model: &str) -> bool {
    model.starts_with(MODEL_PREFIX)
}

/// The model setup gives a session that chooses Claude Code (Steven, 10-03).
pub const DEFAULT_MODEL: &str = "claude-code/sonnet";

/// **Claude Code's effort variable — Shannon's start level** (Steven, 10-06:
/// "`CLAUDE_CODE_EFFORT_LEVEL` should set the level at start"). Launch reads
/// it into `[turn] effort` (`ConfigStack::start_effort`); `claude_command`
/// keeps it from `claude`, where it "takes precedence over `--effort`" (Claude
/// Code's docs) and so silently overruled every session's own level.
pub const EFFORT_VAR: &str = "CLAUDE_CODE_EFFORT_LEVEL";

/// **The variables Shannon never hands on** — its key's
/// (`config::KEY_VARS`) and the effort's ([`EFFORT_VAR`]). The one list:
/// taken off `claude`'s environment (`claude_command`), refused as a name
/// in `[exec] env` and as a `$NAME` in one of its values
/// (`exec::configured_env`) — three hand-chained copies before (review 29).
pub(crate) fn withheld_vars() -> impl Iterator<Item = &'static str> {
    crate::config::KEY_VARS.into_iter().chain([EFFORT_VAR])
}

// ---- Is it ready -----------------------------------------------------------

/// **Whether `claude` can serve a turn** — what setup shows, and what a
/// switch to a `claude-code/…` model asks first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginStatus {
    /// Logged in, by Claude Code's word for how (`claude.ai`, `api_key`, …) —
    /// never an account, never a credential.
    LoggedIn(String),
    /// Installed, and not logged in.
    NotLoggedIn,
    /// No `claude` to run.
    NotInstalled,
    /// No answer Shannon can read — why, in a phrase.
    Unknown(String),
}

impl LoginStatus {
    /// Ready to serve a turn.
    pub fn ready(&self) -> bool {
        matches!(self, LoginStatus::LoggedIn(_))
    }
}

/// How long the login check may take before its answer is "could not tell".
pub const LOGIN_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// **Ask `program` (`claude`) whether it is logged in** — `claude auth
/// status`, which prints JSON and exits 0 when logged in, 1 when not (Claude
/// Code's CLI reference).
///
/// Shannon never touches Claude Code's credentials: Claude Code reads its own
/// and says yes or no. Of what it prints, only `loggedIn` and `authMethod`
/// are read ([`read_login`]); the same JSON can carry the account's email and
/// organisation, which Shannon neither shows nor logs.
///
/// **Blocking**, for at most `timeout`, then killed: it runs when the user
/// asked (setup's `r`, a `:model` switch), and an answer a moment later is
/// worth more there than a screen that says "checking" (`App::claude_login`).
///
/// **The deadline bounds the reading too.** Its output was read after it
/// exited, so a grandchild holding the pipe open held the UI thread past the
/// timeout — 8 s against 1 (review 21, Reviewer B). Now read on a thread, cut
/// off at the deadline, and the whole group killed: it leads its own.
pub fn login_status(program: &Path, env: &ProviderEnv, timeout: Duration) -> LoginStatus {
    use std::io::Read as _;
    let deadline = std::time::Instant::now() + timeout;
    let mut child = match login_command(program, env).spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LoginStatus::NotInstalled,
        Err(e) => {
            return LoginStatus::Unknown(format!("`{}` could not run ({e})", program.display()));
        }
    };
    let late = |mut child: std::process::Child| {
        // SAFETY: the group is the child's, which is unreaped here — so its
        // number has not been reused.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
        LoginStatus::Unknown("it did not answer in time".into())
    };
    let (sent, said) = std::sync::mpsc::channel();
    let stdout = child.stdout.take();
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(stdout) = stdout {
            let _ = stdout.take(LOGIN_OUTPUT_CAP).read_to_string(&mut text);
        }
        let _ = sent.send(text);
    });
    let left = || deadline.saturating_duration_since(std::time::Instant::now());
    let Ok(said) = said.recv_timeout(left()) else {
        return late(child);
    };
    let exit = loop {
        match child.try_wait() {
            Ok(Some(exit)) => break exit,
            Ok(None) if !left().is_zero() => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => return late(child),
            Err(e) => return LoginStatus::Unknown(format!("waiting for it failed ({e})")),
        }
    };
    read_login(exit.code(), &said)
}

/// `claude auth status`, leading its own group (so a deadline ends all of it)
/// and given the environment a turn is ([`claude_command`]): the login it
/// reports must be the one a turn will use.
fn login_command(program: &Path, env: &ProviderEnv) -> std::process::Command {
    let mut command = claude_command(program, env);
    command
        .args(["auth", "status"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command
}

/// More than `claude auth status` prints by a wide margin; past it, the answer
/// is not one Shannon reads.
const LOGIN_OUTPUT_CAP: u64 = 64 * 1024;

/// **`claude`, as Shannon starts it** — both spawns (the login check, a turn)
/// begin here.
///
/// **Without Shannon's key** ([`crate::config::KEY_VARS`]). Inherited, it was
/// handed to Claude Code — which bills an `ANTHROPIC_API_KEY` ahead of the
/// login setup said it uses — and to every Bash command Claude Code runs,
/// where `echo $ANTHROPIC_API_KEY` is a line the gate allows: Shannon's own
/// tool shells start from an empty environment for exactly this reason
/// (`exec.rs`), and these did not (review 21, Reviewer B). **Nor
/// [`EFFORT_VAR`]**: Shannon read it at launch, and the level a turn runs at
/// is the `--effort` it sends.
///
/// **The rest as `env` says** ([`ProviderEnv`], `[claude_code] env`): all of
/// it by default — how the user set Claude Code up — or, `minimal`, only
/// what Shannon's own commands get and Claude Code's own config directory
/// (where its login is); `[exec] env` over either; and what is withheld —
/// a lent secret's variable, `[exec] withhold` — taken off LAST, so nothing
/// puts one back. Claude Code's Bash is handed this environment whole: a
/// lent `GITHUB_TOKEN` left in it was read by `echo $GITHUB_TOKEN`, allowed,
/// because the gate judged `$NAME` against the empty environment Shannon's
/// own commands start from (review stop 14, Reviewer E F1).
///
/// **Found on Shannon's own `PATH`** (`exec::on_own_path`): a command
/// given a `PATH` searches THAT one for its program, so `[exec] env`'s
/// chose which `claude` ran (review 29, Reviewer B).
pub(crate) fn claude_command(program: &Path, env: &ProviderEnv) -> std::process::Command {
    let mut command = std::process::Command::new(crate::exec::on_own_path(program));
    if env.minimal {
        command.env_clear();
        command.envs(crate::exec::child_env(&env.set, &env.withheld));
        if let Some(dir) = std::env::var_os(CONFIG_DIR_VAR) {
            command.env(CONFIG_DIR_VAR, dir);
        }
    } else {
        command.envs(env.set.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    for var in withheld_vars() {
        command.env_remove(var);
    }
    for var in &env.withheld {
        command.env_remove(var);
    }
    command
}

/// Where Claude Code keeps its own setup, when not `~/.claude` — what a
/// `minimal` environment keeps, or `claude` would look for its login
/// elsewhere (`claude_import::claude_home` reads the same).
const CONFIG_DIR_VAR: &str = "CLAUDE_CONFIG_DIR";

/// What `claude auth status` said, from its exit code and output. **The JSON
/// decides**; an exit code alone does not, because a Claude Code too old to
/// have the command also exits 1 — and calling that "not logged in" would send
/// the user to log in to fix an install.
pub fn read_login(code: Option<i32>, stdout: &str) -> LoginStatus {
    let said: Option<Value> = serde_json::from_str(stdout.trim()).ok();
    match said.as_ref().and_then(|v| v["loggedIn"].as_bool()) {
        Some(true) => LoginStatus::LoggedIn(
            said.as_ref()
                .and_then(|v| v["authMethod"].as_str())
                .unwrap_or("an unnamed method")
                .to_string(),
        ),
        Some(false) => LoginStatus::NotLoggedIn,
        None => LoginStatus::Unknown(match code {
            Some(0) => "`claude auth status` answered in a shape Shannon does not read".into(),
            _ => "`claude auth status` did not answer — a Claude Code too old to have it?".into(),
        }),
    }
}

// ---- What the gate judges --------------------------------------------------

/// How Shannon's gate decides one of Claude Code's tool calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Judge {
    /// By the permission rules, which read [`Reading::line`].
    Rules,
    /// Touches nothing outside the turn (`TodoWrite`): allowed unasked,
    /// attended or not.
    Inert,
    /// Allowed unasked by configuration — the web under `[claude_code] web
    /// = "allow"`. Consent given for foreground work: a job with no human
    /// present still holds on it (review 2026-10-10, E1).
    Allow,
    /// **Asked every time**: a tool the rules cannot read — a subagent, an MCP
    /// server, the web by default, a tool newer than this list.
    Ask,
    /// **Asked every time, read and shown**: a `Bash` call asking to run
    /// OUTSIDE Claude Code's own sandbox (`dangerouslyDisableSandbox`). The
    /// rules read its command and nothing of that request, so no allow
    /// answers it; a deny still denies (the 11-07 review, finding 17).
    Unsandboxed,
}

/// **Whether a `Bash` call asks to run outside Claude Code's sandbox** —
/// `dangerouslyDisableSandbox` set to anything but `false`. Failing closed: a
/// value that is not a boolean is not read as "no".
fn leaves_sandbox(input: &Value) -> bool {
    !matches!(
        input.get("dangerouslyDisableSandbox"),
        None | Some(Value::Null | Value::Bool(false))
    )
}

/// **One of Claude Code's tool calls, as Shannon's gate reads it.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// What is judged, shown and recorded. For a tool the rules can read, a
    /// shell line with its effect: `Bash`'s own command; `cat` for a `Read`;
    /// `cat >` for a write; for a `Grep`, `grep -e` of a file or `grep -r` of
    /// a tree, which asks, since a recursive read prints files no argument
    /// names; `find` for a `Glob`.
    /// For any other tool, its name and what it names.
    pub line: String,
    pub judge: Judge,
}

/// **Read a call to `name` with `input`** — Claude Code's tools mapped onto
/// shell lines the rules already judge (`docs/DesignDocs/claude-code-backend.md`;
/// the mapping confirmed by Steven, 10-03). A tool missing what its reading
/// needs asks, as a tool this list does not know does.
///
/// The two questions a reading asks of the machine:
/// - `is_file`: whether a path names a file. A `Grep` of a FILE reads that
///   file and is judged as such (`grep -e`); of anything else — a directory,
///   the cwd by default, a path that cannot be checked — it searches a tree,
///   and may reach hidden files (Claude Code's tools reference does not
///   say), so it reads as `grep -r`,
///   which asks (Steven, 10-04: "drop the `-r`" for the file case).
/// - `lands`: where a path lands through a link, when that is another file
///   (`files::lands`, F2c) — judged too, as Shannon's own file calls are
///   (`files::landed`). One whose name is not text asks. Claude Code opens
///   the file itself, so unlike Shannon's own calls, nothing checks at the
///   run that the link still leads where it was judged.
pub fn reading(
    name: &str,
    input: &Value,
    web: WebPolicy,
    is_file: &dyn Fn(&str) -> bool,
    lands: &dyn Fn(&str) -> Result<Option<String>, String>,
) -> Reading {
    let text = |key: &str| input[key].as_str().filter(|s| !s.is_empty());
    let rules = |line: String| Reading {
        line,
        judge: Judge::Rules,
    };
    let named = |judge: Judge, what: Option<&str>| Reading {
        line: format!(
            "{name} {}",
            what.map_or_else(|| cut(&input.to_string(), 200), str::to_string)
        ),
        judge,
    };
    let web = match web {
        WebPolicy::Ask => Judge::Ask,
        WebPolicy::Allow => Judge::Allow,
    };
    let here = |key: &str| crate::shell::quote(text(key).unwrap_or("."));
    // A line about one file, of its path and of where that lands — or, where
    // the landing cannot be named to the rules, put to a human. A write to a
    // file of several names says its reach too (`files::reach`).
    let of_file =
        |path: &str, act: crate::files::Act, line: &dyn Fn(&str) -> String| match lands(path) {
            Ok(at) => {
                let judged = crate::files::landed(line, path, at.as_deref());
                let file = std::path::Path::new(at.as_deref().unwrap_or(path));
                rules(match crate::files::reach(act, file) {
                    Some(reach) => format!("{judged}; {reach}"),
                    None => judged,
                })
            }
            Err(why) => named(Judge::Ask, Some(format!("{path} ({why})").as_str())),
        };
    // The file tools are read as Shannon's own are (`files::reading`): an
    // Edit is a read as well as a write — it was `cat > path` alone here, so
    // a file whose read is refused could still be edited.
    let file = |act: crate::files::Act, path: &str| {
        of_file(path, act, &|path| crate::files::reading(act, path, None))
    };
    match (name, text("command"), text("file_path")) {
        ("Bash", Some(command), _) if leaves_sandbox(input) => Reading {
            line: command.to_string(),
            judge: Judge::Unsandboxed,
        },
        ("Bash", Some(command), _) => rules(command.to_string()),
        ("Read", _, Some(path)) => file(crate::files::Act::Read, path),
        ("Write", _, Some(path)) => file(crate::files::Act::Write, path),
        ("Edit" | "MultiEdit", _, Some(path)) => file(crate::files::Act::Edit, path),
        ("NotebookEdit", ..) if text("notebook_path").is_some() => file(
            crate::files::Act::Write,
            text("notebook_path").unwrap_or("."),
        ),
        // A file is read, not searched as a tree — of its landing too.
        ("Grep", ..) if text("pattern").is_some() && text("path").is_some_and(is_file) => {
            let path = text("path").unwrap_or(".");
            of_file(path, crate::files::Act::Read, &|path| {
                format!("grep -e {} {}", here("pattern"), crate::shell::quote(path))
            })
        }
        ("Grep", ..) if text("pattern").is_some() => {
            rules(format!("grep -r -e {} {}", here("pattern"), here("path")))
        }
        ("Glob", ..) => rules(format!("find {}", here("path"))),
        ("WebFetch", ..) => named(web, text("url")),
        ("WebSearch", ..) => named(web, text("query")),
        ("TodoWrite", ..) => Reading {
            line: name.to_string(),
            judge: Judge::Inert,
        },
        ("Task" | "Agent", ..) => named(Judge::Ask, text("description")),
        _ => named(Judge::Ask, None),
    }
}

/// The input field `e` edits, for the tools whose edit maps back onto their
/// input: `Bash`'s command, a `Write`'s content, an `Edit`'s new text
/// (Steven, 10-05). Any other tool's line is Shannon's reading of it, which
/// does not map back, so an edit to one is refused rather than guessed at —
/// and so is a `MultiEdit`'s, whose several new texts are not one text.
fn edited_field(name: &str) -> Option<&'static str> {
    match name {
        "Bash" => Some("command"),
        "Write" => Some("content"),
        "Edit" => Some("new_string"),
        _ => None,
    }
}

/// **What `e` opens for `name`'s call**: its command, or the text it writes.
/// `None` for a tool that takes no edit.
pub fn edit_text(name: &str, input: &Value) -> Option<String> {
    input[edited_field(name)?].as_str().map(str::to_string)
}

/// **The input that runs `edited` in place of the call asked for**: the
/// call's input with [`edit_text`]'s field replaced. `None` for a tool that
/// takes no edit.
pub fn edited_input(name: &str, input: &Value, edited: &str) -> Option<Value> {
    let field = edited_field(name)?;
    let mut input = input.clone();
    input
        .as_object_mut()?
        .insert(field.into(), Value::String(edited.to_string()));
    Some(input)
}

/// **What one of Claude Code's file tools would write** — the decision pane
/// shows it, so a write is approved seeing what it writes rather than as
/// `cat > path`, Shannon's reading of it (Steven, 10-05).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A whole file's new content: a `Write`, a `NotebookEdit`'s source.
    Content(String),
    /// Text replaced, old then new: an `Edit`, each of a `MultiEdit`'s.
    Edits(Vec<(String, String)>),
}

/// The file `name` would write and what it would write there, for the
/// tools [`reading`] reads as `cat >`; `None` for any other tool, or one
/// whose input lacks it.
pub fn change(name: &str, input: &Value) -> Option<(String, Change)> {
    let text = |value: &Value, key: &str| value[key].as_str().map(str::to_string);
    let edit = |value: &Value| Some((text(value, "old_string")?, text(value, "new_string")?));
    match name {
        "Write" => Some((
            text(input, "file_path")?,
            Change::Content(text(input, "content")?),
        )),
        "Edit" => Some((text(input, "file_path")?, Change::Edits(vec![edit(input)?]))),
        "MultiEdit" => {
            let edits: Option<Vec<_>> = input["edits"].as_array()?.iter().map(edit).collect();
            Some((text(input, "file_path")?, Change::Edits(edits?)))
        }
        "NotebookEdit" => Some((
            text(input, "notebook_path")?,
            Change::Content(text(input, "new_source").unwrap_or_default()),
        )),
        _ => None,
    }
}

/// `s`, cut to its first `most` characters, marked where cut.
fn cut(s: &str, most: usize) -> String {
    match s.char_indices().nth(most) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

/// How long Claude Code waits for the hook — a human may take minutes over a
/// decision. On timeout a hook "doesn't block the tool call", so the hook
/// keeps its own deadline ([`HOOK_DEADLINE`]) under this, and refuses first.
pub const HOOK_TIMEOUT: Duration = Duration::from_secs(3600);

/// The hook's own deadline: a minute under [`HOOK_TIMEOUT`].
pub const HOOK_DEADLINE: Duration = Duration::from_secs(3540);

/// Claude Code's switch that returns its Bash to the starting directory after
/// every call (its environment-variable reference).
const MAINTAIN_CWD_ENV: &str = "CLAUDE_BASH_MAINTAIN_PROJECT_WORKING_DIR";

/// Where Claude Code makes its temporary files — under `claude-<uid>/` in
/// it — in place of `/tmp` (its environment-variable reference). Set to
/// Shannon's scratch directory, so Claude Code's scratchpad is in an
/// approved place (Steven, 10-06).
const TMPDIR_ENV: &str = "CLAUDE_CODE_TMPDIR";

/// Claude Code's hard off for its CLAUDE.md files — every one: user,
/// project and auto memory (its environment-variable reference). Set when a
/// request says not to read them ([`SendRequest::own_instructions`]).
const NO_CLAUDE_MDS_ENV: &str = "CLAUDE_CODE_DISABLE_CLAUDE_MDS";

/// Claude Code's off for its auto memory: it neither creates nor loads the
/// files (its environment-variable reference). Set when a request says not
/// to keep them ([`SendRequest::own_memory`]).
const NO_AUTO_MEMORY_ENV: &str = "CLAUDE_CODE_DISABLE_AUTO_MEMORY";

/// Claude Code's off for its AUTO compaction only — its manual `/compact`
/// stays (its environment-variable reference). Set when a request says not
/// to compact unasked ([`SendRequest::auto_compact`]).
const NO_AUTO_COMPACT_ENV: &str = "DISABLE_AUTO_COMPACT";

/// Claude Code's off for its background tasks — the `run_in_background` of
/// Bash and of subagents, and the automatic move of a long subagent or a
/// timed-out command to the background (its environment-variable
/// reference). Set when a request says not to run them
/// ([`SendRequest::background_tasks`]): the gate asks only about a call the
/// turn's output names (`question`), and nothing documents that a
/// background task's calls are in that output — one that is not is refused.
const NO_BACKGROUND_TASKS_ENV: &str = "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS";

/// **The calls a turn's output has named**, by id — insert-only for the
/// turn, its sender held by the turn's loop and a receiver by each
/// question. Dropped with the turn, so a question still waiting is refused
/// at once.
type Named = tokio::sync::watch::Receiver<std::collections::HashSet<String>>;

/// **How long a question waits for its call to be named** (B F2, property
/// 4). Nothing documents the order in which Claude Code writes a call to
/// its output and starts the call's hook, so the gate assumes none: a
/// question waits for its call's id, and one never named is refused. On
/// Shannon's side the bytes can arrive after the question too: a line
/// bigger than the pipe holds is read in pieces, and the loop's `select!`
/// reads whichever is ready first. Seconds cover that; the hook's own
/// deadline is an hour.
const NAMED_WITHIN: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(3)
};

/// What the hook tells Claude Code of a call its output never named —
/// read by Claude Code's model, so it says what to do instead.
const UNNAMED_REASON: &str = "Refused by Shannon's gate: this call was not in Claude Code's \
                              output, so it could not be shown to the user. A background task's \
                              calls may not be — run this in the foreground instead.";

/// The gate's open questions, by **serial** ([`AskId`]) — each a reply
/// waiting for [`Backend::answer_tool`]. Shared by every turn; each turn
/// removes its own when it ends, which refuses any left unanswered.
///
/// Not by `tool_use_id`, which the asker supplies: keyed by it, a second
/// question for one id replaced the first's way back (`insert`), an answer
/// went to whichever had asked last, and a turn's ending or a hook's
/// disconnect removed whatever held the id — another turn's question too
/// (review of `buffers.md`, 10-15, Reviewer B F2). A serial is ours, so
/// none of the three can happen.
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>;

/// **Claude Code as a [`Backend`]** — `docs/DesignDocs/claude-code-backend.md`.
pub struct ClaudeCode {
    /// What a turn starts `claude` with.
    launch: Launch,
    /// The model a turn naming none runs.
    default_model: String,
    pending: Pending,
}

/// **What a turn starts `claude` with**, carried to it whole: a field copied
/// out of the backend one at a time is a field one turn can drop, and the
/// scratch directory could be — `send_message` handing `None` on passed
/// every test (review of f50fda5, Reviewer C).
#[derive(Clone)]
struct Launch {
    /// The `claude` program.
    program: PathBuf,
    /// This binary, which Claude Code runs as `<it> hook`.
    hook: PathBuf,
    /// Shannon's scratch directory, as Claude Code's temporary one
    /// ([`TMPDIR_ENV`]) — none, and Claude Code picks its own.
    tmpdir: Option<PathBuf>,
}

impl ClaudeCode {
    /// Run `program` (`claude`), with `hook` (this binary) as its gate.
    pub fn new(program: impl Into<PathBuf>, hook: impl Into<PathBuf>, default_model: &str) -> Self {
        Self {
            launch: Launch {
                program: program.into(),
                hook: hook.into(),
                tmpdir: None,
            },
            default_model: default_model.to_string(),
            pending: Arc::default(),
        }
    }

    /// Claude Code keeps its temporary files under `dir` — Shannon's scratch
    /// directory (`crate::scratch`), an approved place.
    pub fn with_tmpdir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.launch.tmpdir = Some(dir.into());
        self
    }
}

impl Backend for ClaudeCode {
    fn send_message(&self, req: SendRequest, tx: UnboundedSender<StreamMsg>) -> JoinHandle<()> {
        let launch = self.launch.clone();
        let pending = self.pending.clone();
        let model = req
            .model
            .clone()
            .unwrap_or_else(|| self.default_model.clone());
        tokio::spawn(async move {
            let ran = turn(&launch, &model, req, &pending, &tx);
            if let Err(why) = ran.await {
                let _ = tx.send(StreamMsg::Error(BackendError::ClaudeCode(why)));
            }
        })
    }

    fn model(&self) -> &str {
        &self.default_model
    }

    fn answer_tool(&self, ask: &AskId, reply: Reply) -> bool {
        let waiting = lock(&self.pending).remove(&ask.serial());
        waiting.is_some_and(|answer| answer.send(reply).is_ok())
    }
}

/// The pending map, a poisoned lock read through — its entries are plain
/// senders, never half-written.
fn lock(pending: &Pending) -> std::sync::MutexGuard<'_, HashMap<u64, oneshot::Sender<Reply>>> {
    pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `claude --model`'s name for a Shannon model id: what follows the prefix.
fn model_name(model: &str) -> &str {
    model.strip_prefix(MODEL_PREFIX).unwrap_or(model)
}

/// **The `--settings` that puts Shannon's gate in front of every tool**: one
/// PreToolUse hook, every tool (`*`), this binary as `<it> hook`. Hooks merge
/// with the user's own, and deny wins between them (hook probe, 10-03).
///
/// **`|| exit 2`**, because `shannon hook` fails closed only once it RUNS. A
/// binary deleted or moved under a live session (`cargo clean`, an upgrade),
/// or one killed by a signal, leaves the shell exiting 127 or 128+n — which
/// Claude Code treats as non-blocking: the call proceeds, past Shannon's gate
/// (review 21, Reviewer A). The shell's own failure is now a refusal too.
fn settings(hook: &Path) -> String {
    // A hook's `command` is run by a shell, and a path may hold a space or
    // a quote.
    let command = format!(
        "{} hook || exit 2",
        crate::shell::quote(&hook.display().to_string())
    );
    serde_json::json!({
        "hooks": {"PreToolUse": [{"matcher": "*", "hooks": [{
            "type": "command",
            "command": command,
            "timeout": HOOK_TIMEOUT.as_secs(),
        }]}]}
    })
    .to_string()
}

/// `claude`'s arguments for one turn: with `effort`, `--effort <level>`
/// (this turn only; it does not persist in Claude Code's settings).
fn arguments(
    model: &str,
    resume: Option<&str>,
    effort: Option<crate::Effort>,
    settings: &str,
) -> Vec<String> {
    let mut argv: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        // Anything that would prompt is refused: there is no terminal, and
        // the hook has answered for what Shannon allows (hook probe: a hook's
        // allow lifts dontAsk's refusal; the user's own deny rules still hold).
        "--permission-mode",
        "dontAsk",
        "--model",
        model_name(model),
        "--settings",
        settings,
        // What Shannon does around the model (its handles, its manual) —
        // appended, so Claude Code's own prompt stays (`--system-prompt`
        // would replace it); the text Shannon's own turns are given.
        "--append-system-prompt",
        crate::backend::request::shannon_notes(),
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    // **Forked, always**: `--resume` alone appends to the session, so the one
    // a history was filed under would move on, and a branch from an earlier
    // turn would resume turns its transcript does not hold (review 21,
    // Reviewer A). A fork leaves the filed session as it was filed.
    if let Some(id) = resume {
        argv.extend([
            "--resume".to_string(),
            id.to_string(),
            "--fork-session".to_string(),
        ]);
    }
    if let Some(effort) = effort {
        argv.extend(["--effort".to_string(), effort.as_str().to_string()]);
    }
    argv
}

/// **The turn's input, as stream-json lines**: the newest message, a user's.
/// With a session to resume, that is all — Claude Code has the rest. Without
/// one but with earlier messages (a switch to Claude Code mid-conversation),
/// they go first as ONE assistant line: the switch probe found an injected
/// `assistant` line is remembered, and a `user` line would RUN as a turn.
pub(crate) fn turn_input(messages: &[Message], resuming: bool) -> Result<String, String> {
    let (last, earlier) = messages.split_last().ok_or("there is no message to send")?;
    if last.role != Role::User {
        return Err("the newest message is not the user's".into());
    }
    let line = |role: &str, texts: Vec<&str>| {
        let content: Vec<serde_json::Value> = texts
            .into_iter()
            .map(|text| serde_json::json!({"type": "text", "text": text}))
            .collect();
        serde_json::json!({
            "type": role,
            "message": {"role": role, "content": content},
        })
        .to_string()
            + "\n"
    };
    let mut out = String::new();
    if !resuming && !earlier.is_empty() {
        out.push_str(&line("assistant", vec![&handoff(earlier)]));
    }
    // The user's message as the model reads it ([`Message::wire`]): its
    // context ahead of the prompt. It sent `content` alone, which dropped
    // every block the message carried.
    let wire = last.wire();
    let texts: Vec<&str> = match wire.blocks.is_empty() {
        true => vec![wire.content.as_str()],
        false => wire
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect(),
    };
    out.push_str(&line("user", texts));
    Ok(out)
}

/// **How a handoff begins** — and how an import knows one: every session
/// Shannon seeds this way leaves a transcript in Claude Code's directory
/// whose first line is Shannon's own copy of a history it already holds
/// (`claude_import`, which leaves it out).
pub(crate) const HANDOFF: &str =
    "(Earlier in this conversation, served through another backend and recorded by Shannon:)";

/// The earlier turns as text, for the one seeded line — tool calls named
/// with their results, cut short where long.
fn handoff(earlier: &[Message]) -> String {
    let cut = |s: &str| cut(s, 2000);
    let mut out = format!("{HANDOFF}\n");
    for message in earlier {
        let who = match message.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        if !message.content.is_empty() {
            out.push_str(&format!("\n{who}: {}\n", cut(&message.content)));
        }
        for block in &message.blocks {
            match block {
                Block::ToolUse { name, input, .. } => {
                    out.push_str(&format!(
                        "\n[{who} ran {name}: {}]\n",
                        cut(&input.to_string())
                    ));
                }
                Block::ToolResult {
                    content, is_error, ..
                } => {
                    let what = if *is_error {
                        "refused or failed"
                    } else {
                        "result"
                    };
                    out.push_str(&format!("\n[{what}: {}]\n", cut(content)));
                }
                Block::Text { text } => out.push_str(&format!("\n{who}: {}\n", cut(text))),
                Block::Context { kind, source, text } => {
                    out.push_str(&format!(
                        "\n{}\n",
                        Block::context_text(*kind, source, &cut(text))
                    ));
                }
                // The other model's thinking, bound to a conversation this one
                // is not: what it said and did is the history; how it got
                // there is not handed on.
                Block::Thinking { .. } | Block::RedactedThinking { .. } => {}
            }
        }
    }
    out
}

/// **The gate's socket** — opened before `claude` starts, removed when the
/// turn ends. In a fresh owner-only directory: the socket is how a tool call
/// gets allowed, so nobody else may stand at it.
struct Gate {
    dir: PathBuf,
    listener: tokio::net::UnixListener,
}

impl Gate {
    fn open() -> Result<Self, String> {
        let dir = gate_dir()?;
        let listener = tokio::net::UnixListener::bind(dir.join("hook.sock")).map_err(|e| {
            let _ = std::fs::remove_dir_all(&dir);
            format!("could not open the gate's socket ({e}) — nothing ran")
        })?;
        Ok(Self { dir, listener })
    }

    fn socket(&self) -> PathBuf {
        self.dir.join("hook.sock")
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// **The gate's directory, fresh and owner-only.** `0700` is the boundary on
/// a shared `/tmp`: without it another user could unlink `hook.sock` and bind
/// their own, and answer `allow` (review 21, Reviewer B: it was held by
/// nothing). `create` refuses one that exists, so a directory planted at the
/// name is never used.
fn gate_dir() -> Result<PathBuf, String> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    let dir = std::env::temp_dir().join(format!(
        "shannon-hook-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    builder
        .create(&dir)
        .map_err(|e| format!("could not open the gate's directory ({e}) — nothing ran"))?;
    Ok(dir)
}

/// A name no other turn of this process has: a count, so two turns opened in
/// the same instant differ (the clock is microsecond-grained on macOS, and the
/// second turn refused itself — review 21, Reviewer B), and the time, so a
/// name left by an earlier process with this pid does not collide.
fn unique_suffix() -> String {
    static OPENED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = OPENED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{n:x}-{nanos:x}")
}

/// This turn's questions, removed from the shared map when it ends — so any
/// left unanswered are refused (their senders drop), on an abort too. Only
/// its own, by serial: by id, a turn's ending removed another turn's
/// question that happened to name the same call.
struct Asked {
    pending: Pending,
    ids: Questions,
}

/// The questions one turn has put to the gate, as asked — and whether the
/// turn has ended ([`TurnQuestions`]).
type Questions = Arc<Mutex<TurnQuestions>>;

/// **One turn's questions, and whether it has ended**, under one lock with
/// the map's entries they name — so a question cannot be put to the gate
/// after its turn's ending took the list. It was two steps, an insert into
/// the map and a push onto the list: a turn ending between them (question
/// tasks run apart from the turn, on a multi-threaded runtime) left the
/// entry in the map for good (review stop 7, Reviewer E 6). Locked list
/// first, then the map, in [`ask`] and in [`Asked`]'s drop alike.
#[derive(Debug, Default)]
struct TurnQuestions {
    asked: Vec<AskId>,
    ended: bool,
}

impl Drop for Asked {
    fn drop(&mut self) {
        let mut turn = self
            .ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        turn.ended = true;
        let mut pending = lock(&self.pending);
        for ask in std::mem::take(&mut turn.asked) {
            pending.remove(&ask.serial());
        }
    }
}

/// **One `claude -p` turn**: the gate opened, `claude` run under it, its
/// stream sent on, its questions put to the gate. `Err` is a turn that
/// could not run or did not finish; a turn Claude Code reports as failed is
/// sent as a [`StreamMsg::Error`] by the stream.
async fn turn(
    launch: &Launch,
    model: &str,
    req: SendRequest,
    pending: &Pending,
    tx: &UnboundedSender<StreamMsg>,
) -> Result<(), String> {
    let program = &launch.program;
    let input = turn_input(&req.messages, req.resume.is_some())?;
    // **The gate first.** Without its socket the hook refuses every call, but
    // a turn that cannot be gated must not start at all.
    let gate = Gate::open()?;
    let mut command = turn_command(launch, model, &req, &gate.socket());
    let mut child = command.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!(
            "`{}` is not installed or not on PATH — install Claude Code, and run `claude` once to \
             log in",
            program.display()
        ),
        _ => format!("could not start `{}` ({e})", program.display()),
    })?;
    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err("`claude` started without its pipes".into());
    };
    let claude = Claude(Some(crate::exec::Leader::new(child)));
    // **Fed while it is read.** Written first, a long seeded handoff could
    // fill the pipe while `claude` filled its stdout, each waiting on the
    // other (review 21, Reviewer B). Its end closes stdin.
    let mut feeding = tokio::task::JoinSet::new();
    feeding.spawn(async move { stdin.write_all(input.as_bytes()).await });

    let asked = Asked {
        pending: pending.clone(),
        ids: Arc::default(),
    };
    let was_asked = |id: &str| {
        asked
            .ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .asked
            .iter()
            .any(|a| a.tool_use_id() == id)
    };
    // Questions are answered while the stream is read; both end with the turn
    // (a `JoinSet` aborts what it holds when dropped).
    let mut questions = tokio::task::JoinSet::new();
    // What the output has named ([`Named`]): a question is put to the gate
    // only for a call in it, so a process that found the socket cannot ask
    // about a call it made up.
    let (named, _) = tokio::sync::watch::channel(std::collections::HashSet::new());
    // The end of it is the reason.
    let stderr_tail = tokio::spawn(async move { stderr_said(&stderr_kept(stderr).await) });
    let mut lines = BufReader::new(stdout).lines();
    let mut calls = RelayState::default();
    let mut finished: Option<bool> = None;
    // Why `claude` must be stopped rather than waited for.
    let mut stop: Option<String> = None;
    'turn: loop {
        tokio::select! {
            accepted = gate.listener.accept() => {
                if let Ok((stream, _)) = accepted {
                    let ids = asked.ids.clone();
                    questions.spawn(question(stream, pending.clone(), ids, tx.clone(), named.subscribe()));
                }
            }
            line = lines.next_line() => {
                let line = match line {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    // Waited on instead, a `claude` still writing would never
                    // end, and neither would the turn (review 21, Reviewer B).
                    Err(e) => {
                        stop = Some(format!("reading `claude`'s output failed ({e})"));
                        break;
                    }
                };
                match take_line(&line, &named, &mut calls, &was_asked, tx) {
                    Ok(None) => {}
                    Ok(Some(failed)) => finished = Some(failed),
                    Err(tool) => {
                        stop = Some(format!(
                            "`{tool}` ran without passing Shannon's gate, so the turn was \
                             stopped — a Claude Code setting may turn hooks off \
                             (`disableAllHooks`, in this project's .claude/settings*.json or \
                             yours)"
                        ));
                        break 'turn;
                    }
                }
            }
        }
    }
    drop(questions);
    drop(asked);
    let exited = match stop {
        // Interrupted, then its group killed (`Claude`'s drop).
        Some(why) => {
            drop(claude);
            Err(why)
        }
        None => (claude.reap().await)
            .map(|status| status.to_string())
            .map_err(|e| format!("waiting for `claude`: {e}")),
    };
    let unfed = match feeding.try_join_next() {
        Some(Ok(Err(e))) => Some(format!("could not send the turn to `claude` ({e})")),
        _ => None,
    };
    let close = close(finished, exited, unfed);
    let said = match close {
        Close::Exited(_) => stderr_tail.await.unwrap_or_default(),
        _ => String::new(),
    };
    if let Some(end) = ending(close, said) {
        let _ = tx.send(end);
    }
    Ok(())
}

/// **How a turn closes, decided once** — from whether `claude`'s stream
/// finished (`Some(failed)`: its `result` line), how it exited (or why it
/// was stopped), and whether its input could be sent. A stream ends in
/// exactly one `Complete` or `Error`, the one the consumer's turn gate
/// waits for (review stop 12, T9 4): an exit with no `result` must be said,
/// or the turn waits for good; and once a failed `result` has said the
/// turn's `Error`, nothing after it ends the turn again — a second ending
/// would land on a turn already closed.
#[derive(Debug, Clone, PartialEq)]
enum Close {
    /// A failed `result`: the relay sent its `Error`. A later reason is
    /// logged, not sent.
    Said,
    /// A clean `result`: `Complete`.
    Complete,
    /// Stopped, not reaped, or not fed: the reason, as a turn that could not
    /// run is said (`ClaudeCode`).
    Failed(String),
    /// Exited with no `result`: `ClaudeExited`, its status and what it said.
    Exited(String),
}

fn close(finished: Option<bool>, exited: Result<String, String>, unfed: Option<String>) -> Close {
    match (finished, exited, unfed) {
        (Some(true), exited, _) => {
            if let Err(why) = exited {
                tracing::warn!(%why, "after a failed turn, its stream already ended");
            }
            Close::Said
        }
        (_, Err(why), _) => Close::Failed(why),
        (Some(false), Ok(_), _) => Close::Complete,
        (None, Ok(_), Some(why)) => Close::Failed(why),
        (None, Ok(status), None) => Close::Exited(status),
    }
}

/// What a [`Close`] sends: the turn's ending, or nothing — the relay sent it.
fn ending(close: Close, said: String) -> Option<StreamMsg> {
    let failed = match close {
        Close::Said => return None,
        Close::Complete => return Some(StreamMsg::Complete),
        Close::Failed(why) => BackendError::ClaudeCode(why),
        // Structured, its reason whole: the TUI scrubs it before it cuts it
        // (`BackendError::ClaudeExited`).
        Close::Exited(status) => BackendError::ClaudeExited { status, said },
    };
    Some(StreamMsg::Error(failed))
}

/// **The `claude` a turn runs**: its arguments, its gate's socket, its own
/// group, and the environment `req.env` says, holding none of Shannon's key
/// nor anything withheld ([`claude_command`]) — and, when `req` says so, no
/// CLAUDE.md and no memory of its own.
fn turn_command(
    launch: &Launch,
    model: &str,
    req: &SendRequest,
    socket: &Path,
) -> tokio::process::Command {
    // Yours first (`req.env`: what `claude` is given, `[exec] env` over it,
    // the withheld taken off), so what Shannon sets below — its scratch, its
    // switches, its hook — wins.
    let mut command = tokio::process::Command::from(claude_command(&launch.program, &req.env));
    if let Some(dir) = &launch.tmpdir {
        command.env(TMPDIR_ENV, dir);
    }
    if !req.own_instructions {
        command.env(NO_CLAUDE_MDS_ENV, "1");
    }
    if !req.own_memory {
        command.env(NO_AUTO_MEMORY_ENV, "1");
    }
    if !req.auto_compact {
        command.env(NO_AUTO_COMPACT_ENV, "1");
    }
    if !req.background_tasks {
        command.env(NO_BACKGROUND_TASKS_ENV, "1");
    }
    let resume = req.resume.as_deref();
    command
        .args(arguments(
            model,
            resume,
            req.effort,
            &settings(&launch.hook),
        ))
        .env(hook::SOCKET_ENV, socket)
        .env(hook::DEADLINE_ENV, HOOK_DEADLINE.as_secs().to_string())
        // **Every Bash call starts where Shannon did.** Claude Code otherwise
        // keeps a `cd` between calls, and the gate — which judges a line from
        // where Shannon started — read `cat prod_config.txt` after `cd src`
        // as a file at the top: a protected path, allowed (review 21,
        // Reviewer B). A call reported elsewhere is refused (`App`).
        .env(MAINTAIN_CWD_ENV, "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    command
}

/// How long `claude` has to end its own work once interrupted, before its
/// group is killed.
const INTERRUPT_GRACE: Duration = Duration::from_secs(3);

/// **`claude`'s process group, ended the way Claude Code ends its own work.**
///
/// Claude Code starts each Bash command in a group of its own, so a SIGKILL
/// to `claude`'s group never reaches one: an aborted turn left an approved
/// command running, orphaned (observed, review 21, Reviewer B). In `-p` mode
/// SIGINT ends the turn, where SIGTERM leaves it unfinished (Claude Code's
/// headless documentation), and an ended turn ends its shells. So `claude`
/// alone is interrupted first, and the group killed only if it has not ended
/// within [`INTERRUPT_GRACE`]: a reap cut short drops an unreaped
/// [`Leader`](crate::exec::Leader), which kills its group.
struct Claude(Option<crate::exec::Leader>);

impl Claude {
    /// Wait for it to end on its own.
    async fn reap(mut self) -> std::io::Result<std::process::ExitStatus> {
        match self.0.take() {
            Some(leader) => leader.reap().await,
            None => Err(std::io::Error::other("`claude` was already ended")),
        }
    }
}

impl Drop for Claude {
    fn drop(&mut self) {
        let Some(leader) = self.0.take() else {
            return;
        };
        end_gracefully(leader, INTERRUPT_GRACE);
    }
}

/// Interrupt `leader`, give it `grace` to end, then kill its group. Waited
/// for on the runtime, since a drop cannot wait; with no runtime (Shannon
/// exiting) the group is killed at once.
fn end_gracefully(leader: crate::exec::Leader, grace: Duration) {
    leader.interrupt();
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            runtime.spawn(async move {
                let _ = tokio::time::timeout(grace, leader.reap()).await;
            });
        }
        Err(_) => drop(leader),
    }
}

/// What one event did to the turn.
#[derive(Debug, PartialEq)]
enum Relayed {
    On,
    Finished {
        failed: bool,
    },
    /// **A tool ran that the gate was never asked about** — the tool, by
    /// name (or id, when no call came first).
    Ungated(String),
}

/// **One line of `claude`'s output, taken in**: each call it names named
/// to the gate (`named`, which a question waits on), each event relayed.
/// What the turn must act on comes back: `Some(failed)` at the turn's
/// close, or `Err` with the tool that ran ungated — the turn stops there,
/// and the line's later events are not relayed. The loop's whole work for
/// a line, here so a test can hand it lines: the gate's socket cannot be
/// bound where this suite usually runs.
fn take_line(
    line: &str,
    named: &tokio::sync::watch::Sender<std::collections::HashSet<String>>,
    state: &mut RelayState,
    was_asked: &dyn Fn(&str) -> bool,
    tx: &UnboundedSender<StreamMsg>,
) -> Result<Option<bool>, String> {
    // The error names a position, never the line's content.
    let events = read_line(line).unwrap_or_else(|why| {
        tracing::warn!(%why, "a line from `claude` was skipped");
        Vec::new()
    });
    let mut finished = None;
    for event in events {
        if let Some(id) = event.names_a_call() {
            named.send_modify(|ids| {
                ids.insert(id.to_string());
            });
        }
        match relay(event, state, was_asked, tx) {
            Relayed::On => {}
            Relayed::Finished { failed } => finished = Some(failed),
            Relayed::Ungated(tool) => return Err(tool),
        }
    }
    Ok(finished)
}

/// What a turn's relay remembers between lines.
#[derive(Debug, Default)]
struct RelayState {
    /// Calls awaiting their results, by id: a record is sent whole.
    calls: HashMap<String, (String, Value)>,
    /// Whether the stream has carried a request's own usage. Then the
    /// `result` line's — the total over every request of the run (the Agent
    /// SDK documents a result's totals as cumulative) — is not sent: as one
    /// reading it would read as one request, every round's prompt added up.
    /// It stays the fallback for a stream that carried none.
    metered: bool,
}

/// **One event, sent on as what Shannon reads** — a session, text, a record
/// once its result is in (a call is kept until then), or the turn's close.
///
/// **And the gate is shown to be in the loop.** Configured is not present: a
/// `"disableAllHooks": true` in any of Claude Code's settings files turns
/// off every hook, the `--settings` one too — the key is read after
/// settings precedence, and `--settings` overrides only the keys it names
/// (Claude Code's hooks and settings documentation) — and then every call
/// ran unasked while the screen said Shannon decides them (review 21,
/// Reviewer B). A call's question always reaches the gate before its result
/// can exist, so a SUCCESSFUL result for an id never asked is a call that
/// bypassed it. Only a success: Claude Code also answers some calls with an
/// error before any hook runs (an unknown tool, a bad input, a cancel), and a
/// failed command is an error too — so an ungated call that fails slips past,
/// and the next one that works does not.
fn relay(
    event: Event,
    state: &mut RelayState,
    was_asked: &dyn Fn(&str) -> bool,
    tx: &UnboundedSender<StreamMsg>,
) -> Relayed {
    let calls = &mut state.calls;
    match event {
        Event::Session(id) => {
            let _ = tx.send(StreamMsg::ProviderSession(id));
        }
        Event::Servers(servers) => {
            let _ = tx.send(StreamMsg::ProviderServers(servers));
        }
        Event::Compacted { auto, before } => {
            let _ = tx.send(StreamMsg::ProviderCompacted { auto, before });
        }
        Event::Text(text) => {
            let _ = tx.send(StreamMsg::Chunk(text));
        }
        Event::Thinking(text) => {
            let _ = tx.send(StreamMsg::ThinkingChunk(text));
        }
        Event::Thought(block) => {
            let _ = tx.send(StreamMsg::Thought(block));
        }
        // Each request's own books: the one before is closed first.
        Event::Request(usage) => {
            state.metered = true;
            let _ = tx.send(StreamMsg::NextRequest);
            let _ = tx.send(StreamMsg::Usage(usage));
        }
        Event::RequestUsage(usage) => {
            let _ = tx.send(StreamMsg::Usage(usage));
        }
        // Named to the gate by the turn's loop; nothing to send.
        Event::ToolNamed(_) => {}
        Event::ToolCalled {
            id, name, input, ..
        } => {
            calls.insert(id, (name, input));
        }
        Event::ToolReturned {
            id,
            output,
            is_error,
            ..
        } => {
            let ungated = !is_error && !was_asked(&id);
            // A result with no call before it is not a record: kept as one it
            // would be a nameless `tool_use` a later API request is refused
            // for (review 21, Reviewer A).
            let Some((name, input)) = calls.remove(&id) else {
                tracing::debug!(id, "a tool result with no call before it; not recorded");
                return if ungated {
                    Relayed::Ungated(id)
                } else {
                    Relayed::On
                };
            };
            // Recorded either way: an ungated call still ran.
            let tool = name.clone();
            let _ = tx.send(StreamMsg::ToolRecord {
                call: Block::ToolUse {
                    id: id.clone(),
                    name,
                    input,
                },
                result: Block::ToolResult {
                    tool_use_id: id,
                    content: output,
                    is_error,
                },
            });
            if ungated {
                return Relayed::Ungated(tool);
            }
        }
        Event::Finished {
            stop,
            usage,
            failed,
            summary,
        } => {
            if !state.metered {
                let _ = tx.send(StreamMsg::Usage(usage));
            }
            let _ = tx.send(StreamMsg::Stopped(stop));
            if failed {
                let said = if summary.is_empty() {
                    "the turn failed".to_string()
                } else {
                    summary
                };
                let _ = tx.send(StreamMsg::Error(BackendError::ClaudeCode(said)));
            }
            return Relayed::Finished { failed };
        }
    }
    Relayed::On
}

/// The most a question may hold: a `Write` carries a whole file, so far more
/// than any line, and far less than memory. Small under test, so a test can
/// reach it.
const QUESTION_CAP: u64 = if cfg!(test) { 4096 } else { 64 * 1024 * 1024 };

/// How long a connection has to send its question. The hook writes it as it
/// connects (`hook::decide`). Short under test, as above.
const QUESTION_ARRIVES: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(10)
};

/// **One question from the hook**: read it, put it to the gate as a
/// [`StreamMsg::ToolAsk`], wait for [`Backend::answer_tool`], answer. Every
/// way of not getting an answer is a refusal — the hook would refuse anyway.
///
/// **Only about a call the turn's output named** (`named`; B F2, property
/// 4). Every shell Claude Code starts inherits the socket's path, so a
/// question can come from a process that made its call up; one the output
/// has not named within [`NAMED_WITHIN`] is refused, said to the TUI
/// ([`StreamMsg::ToolUnnamed`]), and never reaches the pane. A background
/// agent's calls are never named (Steven, 10-22: kept, and refused —
/// `[claude_code] background_tasks = false` runs that work in the
/// foreground). It stops a made-up id, not a real one read from the
/// transcript: what holds that to one refused call is the gate's rule that
/// two questions about one call are both refused.
async fn question(
    stream: tokio::net::UnixStream,
    pending: Pending,
    ids: Questions,
    tx: UnboundedSender<StreamMsg>,
    mut named: Named,
) {
    // **When the hook asked**: its own deadline runs from its connecting, a
    // moment before this ran, so what the question says is left of it
    // ([`ToolQuestion::waits`]) is measured from here.
    let asked_at = std::time::Instant::now();
    let (read, mut write) = stream.into_split();
    // **Bounded.** Any local process that finds the socket can connect —
    // Claude Code's tool shells inherit its path — so a question that never
    // arrives, or never ends, must not hold a task or memory (review 21,
    // Reviewer B). Past the cap the line is cut, so it does not parse.
    let mut reader = BufReader::new(read.take(QUESTION_CAP));
    let mut line = String::new();
    let arrived = tokio::time::timeout(QUESTION_ARRIVES, reader.read_line(&mut line)).await;
    let reply = match arrived.map(|_| serde_json::from_str::<hook::Request>(&line)) {
        Err(_) => Reply::Deny {
            reason: "the hook's question did not arrive in time".into(),
        },
        Ok(Ok(hook::Request { call })) if !named_in_time(&mut named, &call).await => {
            let (id, name) = (call["tool_use_id"].as_str(), call["tool_name"].as_str());
            let _ = tx.send(StreamMsg::ToolUnnamed {
                id: id.unwrap_or_default().to_string(),
                name: name.unwrap_or("a tool").to_string(),
            });
            Reply::Deny {
                reason: UNNAMED_REASON.into(),
            }
        }
        Ok(Ok(hook::Request { call })) => {
            // **Answered — or the hook is gone.** Claude Code kills a hook (a
            // sibling call failed, its own timeout), and an answer then reaches
            // nobody, while the question sat on Shannon's pane blocking every
            // prompt — and answering it recorded "runs" for nothing (review
            // 21, Reviewer A). The hook sends one line and then only reads, so
            // the next read here ends only when the hook does. What leaves is
            // THIS question, by its serial — by id, a connection that went
            // away took with it another's question naming the same call.
            let mut rest = [0u8; 1];
            let asked = asked_of(&call);
            tokio::select! {
                reply = ask(call, asked.clone(), asked_at, &pending, &ids, &tx) => reply,
                _ = reader.read(&mut rest) => {
                    if let Some(asked) = asked {
                        lock(&pending).remove(&asked.serial());
                        let _ = tx.send(StreamMsg::ToolWithdrawn(asked));
                    }
                    return;
                }
            }
        }
        Ok(Err(_)) => Reply::Deny {
            reason: "the hook's question did not parse".into(),
        },
    };
    if let Ok(mut text) = serde_json::to_string(&reply) {
        text.push('\n');
        let _ = write.write_all(text.as_bytes()).await;
    }
}

/// **Has the turn's output named `call`** — at once, or within
/// [`NAMED_WITHIN`]? A turn that ended (its sender gone) names nothing
/// more. A question naming no call passes: [`ask`] refuses it, saying so.
async fn named_in_time(named: &mut Named, call: &Value) -> bool {
    let Some(id) = call["tool_use_id"].as_str() else {
        return true;
    };
    let seen = tokio::time::timeout(NAMED_WITHIN, named.wait_for(|ids| ids.contains(id)));
    matches!(seen.await, Ok(Ok(_)))
}

/// **A hook's question, given its serial** ([`AskId::mint`]) — `None` for one
/// naming no call.
fn asked_of(call: &Value) -> Option<AskId> {
    call["tool_use_id"].as_str().map(AskId::mint)
}

/// How much of `claude`'s stderr is kept while it runs — the end, where its
/// reason is. The reader cuts it to a sentence after it scrubs it
/// ([`BackendError::ClaudeExited`]).
const STDERR_KEPT: usize = 16 * 1024;

/// **The end of `claude`'s stderr, kept as it arrives** — its last
/// [`STDERR_KEPT`] bytes, and whether any went (the head drained). It was
/// read whole to the end, so a `claude` that writes without end grew this
/// without end (full review 10-21, B 6).
async fn stderr_kept(mut stderr: impl tokio::io::AsyncRead + Unpin) -> (Vec<u8>, bool) {
    let mut kept: Vec<u8> = Vec::new();
    let mut drained = false;
    let mut chunk = [0u8; 4096];
    while let Ok(n) = stderr.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        kept.extend_from_slice(&chunk[..n]);
        if kept.len() > STDERR_KEPT {
            kept.drain(..kept.len() - STDERR_KEPT);
            drained = true;
        }
    }
    (kept, drained)
}

/// **What `claude` said at the end of its stderr** — what was kept, whole
/// from a line boundary: when the head was drained, the line it cut into
/// goes too. Not cut to a sentence here: the TUI, which knows the secrets,
/// scrubs it first ([`BackendError::ClaudeExited`]). Cut here, 600
/// characters from the end before any scrub, a `password=` pair whose name
/// fell before the cut left its value unmatched (full review 10-21, E 3,
/// probed); cut by bytes before that, a cut inside a multibyte character
/// read as no reason at all (B 6).
fn stderr_said((kept, drained): &(Vec<u8>, bool)) -> String {
    let text = String::from_utf8_lossy(kept);
    let whole = if *drained {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        &text
    };
    whole.trim().to_string()
}

/// Put one call to the gate, as the question `asked` — the hook asking at
/// `asked_at` — and wait for its answer.
async fn ask(
    call: Value,
    asked: Option<AskId>,
    asked_at: std::time::Instant,
    pending: &Pending,
    ids: &Questions,
    tx: &UnboundedSender<StreamMsg>,
) -> Reply {
    let refuse = |reason: &str| Reply::Deny {
        reason: reason.to_string(),
    };
    let (Some(asked), Some(name)) = (asked, call["tool_name"].as_str()) else {
        return refuse("the call named no tool or no id");
    };
    let (answer, answered) = oneshot::channel();
    {
        // The turn's list, then the map, as `Asked`'s drop takes them: a turn
        // that has ended takes no more questions ([`TurnQuestions`]).
        let mut turn = ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if turn.ended {
            return refuse("the turn ended before Shannon answered");
        }
        // Its own serial: nothing here can replace another question's way back.
        //
        // **Both under the one lock, and only the shape of this block holds
        // that**: a test runs these in order and cannot interleave a turn's
        // end between them, so splitting the insert from the push passes
        // every test and brings back the leak (full review 10-21, C 11; review
        // stop 7, E 6).
        lock(pending).insert(asked.serial(), answer);
        turn.asked.push(asked.clone());
    }
    let message = StreamMsg::ToolAsk(ToolQuestion {
        id: asked.tool_use_id().to_string(),
        name: name.to_string(),
        input: call["tool_input"].clone(),
        cwd: call["cwd"].as_str().map(str::to_string),
        ask: asked.clone(),
        // The hook's own deadline, where it refuses.
        answer_by: asked_at.checked_add(HOOK_DEADLINE),
    });
    if tx.send(message).is_err() {
        lock(pending).remove(&asked.serial());
        return refuse("nobody is there to decide (Shannon's turn is gone)");
    }
    answered
        .await
        .unwrap_or_else(|_| refuse("the turn ended before Shannon answered"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::contract::the_ending;

    /// **What stderr said is kept whole, from a line** — not cut to a
    /// sentence before the TUI can scrub it: a `password=` pair 600
    /// characters from the end kept its name and its value together (it was
    /// cut between them, full review 10-21, E 3). A head the drain cut into
    /// loses the rest of its line, never half of one; a multibyte character
    /// is never split (B 6).
    #[test]
    fn stderr_said_is_whole_from_a_line() {
        let pair = "password=hunter2seekrit";
        let after = "x".repeat(599);
        let text = format!("{pair} é\n{after}");
        let said = stderr_said(&(text.clone().into_bytes(), false));
        assert_eq!(said, text, "undrained: all of it, the pair whole");
        // Drained into the first line: that line goes, the rest is whole.
        let drained = format!("word=cut-into\n{pair}\n{after}");
        let said = stderr_said(&(drained.into_bytes(), true));
        assert_eq!(said, format!("{pair}\n{after}"), "from the next line");
        assert_eq!(
            stderr_said(&(b"  short\n".to_vec(), false)),
            "short",
            "trimmed"
        );
    }

    /// **Stderr is kept bounded, by its end** (full review 10-21, B 6): a
    /// `claude` that writes without end kept all of it.
    #[tokio::test]
    async fn stderr_is_kept_bounded_by_its_end() {
        let written: Vec<u8> = (0..STDERR_KEPT * 4).map(|i| (i % 251) as u8).collect();
        let (kept, drained) = stderr_kept(written.as_slice()).await;
        assert_eq!(kept.len(), STDERR_KEPT, "bounded");
        assert_eq!(kept, written[written.len() - STDERR_KEPT..], "by its end");
        assert!(drained, "and says its head went");
        let short = stderr_kept(&b"brief"[..]).await;
        assert_eq!(short, (b"brief".to_vec(), false), "a short one, whole");
    }

    /// A turn with one Bash call, in the shapes the switch probe measured
    /// (10-03) — trimmed and with made-up ids; the recording itself carries
    /// the prober's paths and plugins, so it is not checked in.
    const TURN: &str = r#"
{"type":"system","subtype":"init","session_id":"sess-1","model":"claude-haiku-4-5","permissionMode":"dontAsk"}
{"type":"stream_event","session_id":"sess-1","parent_tool_use_id":null,"event":{"type":"message_start","message":{}}}
{"type":"stream_event","session_id":"sess-1","parent_tool_use_id":null,"event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":""}}}
{"type":"assistant","session_id":"sess-1","parent_tool_use_id":null,"message":{"role":"assistant","content":[{"type":"thinking","thinking":"","signature":"SIG"}]}}
{"type":"assistant","session_id":"sess-1","parent_tool_use_id":null,"message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"echo hi","description":"say hi"}}]}}
{"type":"user","session_id":"sess-1","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","is_error":false,"content":"hi"}]},"tool_use_result":{"stdout":"hi","stderr":""}}
{"type":"stream_event","session_id":"sess-1","parent_tool_use_id":null,"event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"It said "}}}
{"type":"stream_event","session_id":"sess-1","parent_tool_use_id":null,"event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"hi."}}}
{"type":"assistant","session_id":"sess-1","parent_tool_use_id":null,"message":{"role":"assistant","content":[{"type":"text","text":"It said hi."}]}}
{"type":"rate_limit_event","session_id":"sess-1","rate_limit_info":{}}
{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","num_turns":2,"session_id":"sess-1","result":"It said hi.","usage":{"input_tokens":26,"output_tokens":340,"cache_read_input_tokens":71718,"cache_creation_input_tokens":16004}}
"#;

    fn read_all(text: &str) -> Vec<Event> {
        text.lines()
            .flat_map(|l| read_line(l).expect("a line"))
            .collect()
    }

    /// **Claude Code's `init` names its MCP servers** — read beside its
    /// session, so Shannon lists them without reading Claude Code's config.
    #[test]
    fn init_names_the_mcp_servers() {
        let line = r#"{"type":"system","subtype":"init","session_id":"s","mcp_servers":[{"name":"github","status":"connected"},{"name":"slack","status":"failed"},{"status":"pending"}]}"#;
        assert_eq!(
            read_all(line),
            vec![
                Event::Session("s".into()),
                Event::Servers(vec![
                    crate::McpServer {
                        name: "github".into(),
                        status: "connected".into()
                    },
                    crate::McpServer {
                        name: "slack".into(),
                        status: "failed".into()
                    },
                ]),
            ],
            "a server with no name is none"
        );
    }

    /// **Claude Code's thinking is read as its text is** — streamed for the
    /// screen when it has any, the block whole from its `assistant` line;
    /// the top-level agent's only, a subagent's being its own conversation.
    #[test]
    fn claude_codes_thinking_is_read_as_its_text_is() {
        let lines = r#"{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hm"}}}
{"type":"stream_event","parent_tool_use_id":"toolu_task","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"inner"}}}
{"type":"assistant","parent_tool_use_id":"toolu_task","message":{"content":[{"type":"thinking","thinking":"inner","signature":"X"}]}}
{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"redacted_thinking","data":"R"}]}}"#;
        assert_eq!(
            read_all(lines),
            vec![
                Event::Thinking("hm".into()),
                Event::Thought(Block::RedactedThinking {
                    data: "R".into(),
                    at: 0
                }),
            ]
        );
    }

    #[test]
    fn a_turn_reads_as_session_record_text_and_finish() {
        let events = read_all(TURN);
        assert_eq!(
            events,
            vec![
                Event::Session("sess-1".into()),
                // Empty, as Claude Code's default shows it: nothing streamed,
                // the block kept whole.
                Event::Thought(Block::Thinking {
                    thinking: String::new(),
                    signature: "SIG".into(),
                    at: 0,
                }),
                Event::ToolCalled {
                    id: "toolu_1".into(),
                    name: "Bash".into(),
                    input: serde_json::json!({"command": "echo hi", "description": "say hi"}),
                    parent: None,
                },
                Event::ToolReturned {
                    id: "toolu_1".into(),
                    output: "hi".into(),
                    is_error: false,
                    parent: None,
                },
                Event::Text("It said ".into()),
                Event::Text("hi.".into()),
                Event::Finished {
                    stop: StopReason::EndTurn,
                    usage: Usage {
                        input_tokens: 26,
                        output_tokens: 340,
                        cache_creation_input_tokens: 16004,
                        cache_read_input_tokens: 71718,
                    },
                    failed: false,
                    summary: "It said hi.".into(),
                },
            ]
        );
    }

    /// **The final `assistant` text block is not the text twice** — it
    /// repeats what the deltas already streamed.
    #[test]
    fn text_comes_from_the_deltas_only() {
        let text: String = read_all(TURN)
            .into_iter()
            .filter_map(|e| match e {
                Event::Text(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(text, "It said hi.");
    }

    /// A subagent's tool calls are recorded under their parent; its streamed
    /// text is not the reply.
    #[test]
    fn a_subagents_lines_carry_their_parent() {
        let events = read_all(
            r#"{"type":"assistant","parent_tool_use_id":"toolu_task","message":{"content":[{"type":"tool_use","id":"toolu_2","name":"Bash","input":{}}]}}
{"type":"stream_event","parent_tool_use_id":"toolu_task","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"inner"}}}"#,
        );
        assert_eq!(
            events,
            vec![Event::ToolCalled {
                id: "toolu_2".into(),
                name: "Bash".into(),
                input: serde_json::json!({}),
                parent: Some("toolu_task".into()),
            }]
        );
    }

    /// A refused call's result, an array of blocks, and a failed turn.
    #[test]
    fn refusals_block_lists_and_failures_read_as_such() {
        let events = read_all(
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","is_error":true,"content":[{"type":"text","text":"Shannon refused this tool call: rm"},{"type":"image"}]}]}}
{"type":"result","subtype":"error_max_turns","is_error":false,"result":"","usage":{}}"#,
        );
        assert_eq!(
            events[0],
            Event::ToolReturned {
                id: "t".into(),
                output: "Shannon refused this tool call: rm\n[image]".into(),
                is_error: true,
                parent: None,
            }
        );
        assert!(
            matches!(&events[1], Event::Finished { failed: true, .. }),
            "a subtype other than success is a failed turn: {:?}",
            events[1]
        );
    }

    // ---- the backend ------------------------------------------------------

    fn lines_of(input: &str) -> Vec<Value> {
        input
            .lines()
            .map(|l| serde_json::from_str(l).expect("a JSON line"))
            .collect()
    }

    /// **A resumed session gets the new message only; a switch seeds ONE
    /// assistant line, then the message** — a `user` line would RUN as a
    /// turn of its own (the switch probe, 10-03).
    #[test]
    fn the_input_is_the_new_message_and_a_seed_when_switching() {
        let mut answered = Message::assistant("it is 4");
        answered.blocks.push(Block::bash_call("t1", "echo 4"));
        let history = vec![
            Message::user("what is 2+2"),
            answered,
            Message::user("and 3+3?"),
        ];

        let resumed = lines_of(&turn_input(&history, true).expect("input"));
        assert_eq!(resumed.len(), 1, "{resumed:?}");
        assert_eq!(resumed[0]["type"], "user");
        assert_eq!(resumed[0]["message"]["content"][0]["text"], "and 3+3?");

        let switched = lines_of(&turn_input(&history, false).expect("input"));
        assert_eq!(switched.len(), 2, "{switched:?}");
        assert_eq!(switched[0]["type"], "assistant");
        let seed = switched[0]["message"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        for said in [
            "User: what is 2+2",
            "Assistant: it is 4",
            "ran bash",
            "echo 4",
        ] {
            assert!(seed.contains(said), "{said:?} in {seed}");
        }
        assert!(!seed.contains("and 3+3"), "the new message is not history");
        assert_eq!(switched[1]["message"]["content"][0]["text"], "and 3+3?");

        let fresh = lines_of(&turn_input(&[Message::user("hi")], false).expect("input"));
        assert_eq!(fresh.len(), 1, "no history, no seed");

        assert!(turn_input(&[], false).is_err());
        assert!(turn_input(&[Message::user("a"), Message::assistant("b")], true).is_err());
    }

    /// **What Shannon tells the model reaches `claude` too** — the new
    /// message's context ahead of its prompt, and an earlier one in the seed.
    /// It sent `content` alone, and every block on the message was dropped.
    #[test]
    fn the_input_carries_the_context_ahead_of_the_prompt() {
        let told = |text: &str| {
            let mut m = Message::user(text);
            m.blocks.push(Block::Context {
                kind: crate::ContextKind::Instructions,
                source: "/p/AGENTS.md".into(),
                text: format!("rules for {text}"),
            });
            m
        };
        let history = vec![told("first"), Message::assistant("ok"), told("second")];
        let resumed = lines_of(&turn_input(&history, true).expect("input"));
        let content = &resumed[0]["message"]["content"];
        let first = content[0]["text"].as_str().unwrap_or_default();
        assert!(
            first.contains("rules for second") && first.contains("/p/AGENTS.md"),
            "{first}"
        );
        assert_eq!(content[1]["text"], "second");

        let switched = lines_of(&turn_input(&history, false).expect("input"));
        let seed = switched[0]["message"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        assert!(seed.contains("rules for first"), "{seed}");
    }

    /// **Shannon's hook before every tool**, from this binary however its
    /// path is spelled, its deadline under Claude Code's timeout.
    #[test]
    fn the_settings_put_the_hook_before_every_tool() {
        let said: Value =
            serde_json::from_str(&settings(Path::new("/opt/my app/shan'non"))).expect("JSON");
        let group = &said["hooks"]["PreToolUse"][0];
        assert_eq!(group["matcher"], "*");
        assert_eq!(group["hooks"][0]["type"], "command");
        assert_eq!(
            group["hooks"][0]["command"],
            r"'/opt/my app/shan'\''non' hook || exit 2"
        );
        assert_eq!(group["hooks"][0]["timeout"], HOOK_TIMEOUT.as_secs());
        assert!(HOOK_DEADLINE < HOOK_TIMEOUT, "the hook must refuse first");
    }

    /// **A hook that cannot run still refuses** — run as Claude Code runs it,
    /// through a shell. A missing binary exits 127, which Claude Code lets
    /// through; the command must turn it into 2. And it must not turn an
    /// allow (0) or a refusal (2) into anything else.
    #[test]
    fn a_hook_that_cannot_run_refuses() {
        let run = |hook: &str| {
            let said: Value = serde_json::from_str(&settings(Path::new(hook))).expect("JSON");
            let command = said["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
                .as_str()
                .expect("a command")
                .to_string();
            std::process::Command::new("sh")
                .args(["-c", &command])
                .status()
                .expect("sh runs")
                .code()
        };
        assert_eq!(
            run("/nonexistent/shannon-gone"),
            Some(2),
            "a missing binary"
        );
        assert_eq!(run("/usr/bin/true"), Some(0), "an allow stays an allow");
        let dir = tempfile::tempdir().expect("dir");
        let refuses = fake_claude(dir.path(), "exit 2");
        assert_eq!(
            run(&refuses.display().to_string()),
            Some(2),
            "a refusal stays one"
        );
    }

    #[test]
    fn the_arguments_name_the_model_and_the_session() {
        let argv = arguments(
            "claude-code/sonnet",
            Some("sess-1"),
            Some(crate::Effort::Max),
            "{}",
        );
        let after = |flag: &str| {
            argv.iter()
                .position(|a| a == flag)
                .and_then(|i| argv.get(i + 1))
                .cloned()
        };
        assert_eq!(after("--model").as_deref(), Some("sonnet"));
        assert_eq!(after("--resume").as_deref(), Some("sess-1"));
        assert!(
            argv.contains(&"--fork-session".to_string()),
            "a resumed session is forked, so the filed one stays as filed"
        );
        assert_eq!(after("--permission-mode").as_deref(), Some("dontAsk"));
        assert_eq!(after("--settings").as_deref(), Some("{}"));
        // Told what Shannon does around it, as Shannon's own turns are — and
        // appended: `--system-prompt` would drop Claude Code's own.
        assert_eq!(
            after("--append-system-prompt").as_deref(),
            Some(crate::backend::request::shannon_notes())
        );
        assert!(!argv.contains(&"--system-prompt".to_string()));
        assert_eq!(after("--effort").as_deref(), Some("max"));
        let bare = arguments("claude-code/opus", None, None, "{}");
        assert!(!bare.contains(&"--resume".to_string()));
        assert!(
            !bare.contains(&"--effort".to_string()),
            "no effort asked: Claude Code's own"
        );
        assert!(is_claude_code("claude-code/haiku") && !is_claude_code("claude-sonnet-4"));
    }

    /// **Each request is metered on its own, and the summed `result` is not
    /// sent** — Steven, 10-15: `ctx 100%` on a 1M window, every tool round's
    /// prompt added up as one request. A subagent's request fills its own
    /// window and is not one of the turn's; a turn whose stream carried no
    /// usage still bills its `result`.
    #[test]
    fn the_relay_meters_each_request_and_not_their_sum() {
        const ROUNDS: &str = r#"{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"message_start","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":99990,"output_tokens":1}}}}
{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":500}}}
{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"message_start","message":{"usage":{"input_tokens":5,"cache_read_input_tokens":100995}}}}
{"type":"stream_event","parent_tool_use_id":"toolu_9","event":{"type":"message_start","message":{"usage":{"input_tokens":7000}}}}
{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"message_delta","usage":{"output_tokens":300}}}
{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","usage":{"input_tokens":15,"cache_read_input_tokens":200985,"output_tokens":800}}"#;
        let sent = |text: &str| {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let mut state = RelayState::default();
            for event in read_all(text) {
                relay(event, &mut state, &|_: &str| true, &tx);
            }
            let mut out = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                out.push(msg);
            }
            out
        };
        let reading = |input, cached, output| {
            StreamMsg::Usage(Usage {
                input_tokens: input,
                output_tokens: output,
                cache_read_input_tokens: cached,
                ..Default::default()
            })
        };
        assert_eq!(
            sent(ROUNDS),
            vec![
                StreamMsg::NextRequest,
                reading(10, 99_990, 1),
                reading(0, 0, 500),
                StreamMsg::NextRequest,
                reading(5, 100_995, 0),
                reading(0, 0, 300),
                StreamMsg::Stopped(StopReason::EndTurn),
            ]
        );
        let unmetered = ROUNDS.lines().last().expect("the result line");
        assert_eq!(
            sent(unmetered),
            vec![
                reading(15, 200_985, 800),
                StreamMsg::Stopped(StopReason::EndTurn),
            ],
            "no request's usage in the stream: the result is the bill"
        );
    }

    /// **Claude Code compacting its own context is relayed, not dropped** —
    /// its `compact_boundary` meant nothing here, so its model could start
    /// working from a summary with nothing said (Steven, 10-15).
    #[test]
    fn a_compact_boundary_is_relayed() {
        const AUTO: &str = r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"auto","pre_tokens":912345}}"#;
        const MANUAL: &str = r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"manual","pre_tokens":40}}"#;
        assert_eq!(
            read_line(AUTO).unwrap(),
            vec![Event::Compacted {
                auto: true,
                before: 912_345
            }]
        );
        assert_eq!(
            read_line(MANUAL).unwrap(),
            vec![Event::Compacted {
                auto: false,
                before: 40
            }]
        );
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = RelayState::default();
        for event in read_line(AUTO).unwrap() {
            relay(event, &mut state, &|_: &str| true, &tx);
        }
        assert_eq!(
            rx.try_recv().ok(),
            Some(StreamMsg::ProviderCompacted {
                auto: true,
                before: 912_345
            })
        );
    }

    /// **A record is sent once its result is in**, the turn's close as usage,
    /// stop, and — for a failed turn — the failure.
    #[test]
    fn the_relay_sends_records_with_their_results() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut calls = RelayState::default();
        let mut finished = None;
        let asked = |_: &str| true;
        for event in read_all(TURN) {
            match relay(event, &mut calls, &asked, &tx) {
                Relayed::On => {}
                Relayed::Finished { failed } => finished = Some(failed),
                Relayed::Ungated(tool) => panic!("{tool} was asked"),
            }
        }
        assert_eq!(finished, Some(false));
        let mut sent = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            sent.push(msg);
        }
        assert_eq!(sent[0], StreamMsg::ProviderSession("sess-1".into()));
        assert!(
            matches!(&sent[1], StreamMsg::Thought(Block::Thinking { signature, .. }) if signature == "SIG"),
            "the thinking, kept: {:?}",
            sent[1]
        );
        assert_eq!(
            sent[2],
            StreamMsg::ToolRecord {
                call: Block::ToolUse {
                    id: "toolu_1".into(),
                    name: "Bash".into(),
                    input: serde_json::json!({"command": "echo hi", "description": "say hi"}),
                },
                result: Block::ToolResult {
                    tool_use_id: "toolu_1".into(),
                    content: "hi".into(),
                    is_error: false,
                },
            }
        );
        assert!(matches!(
            sent[sent.len() - 1],
            StreamMsg::Stopped(StopReason::EndTurn)
        ));
        assert!(!sent.iter().any(|m| matches!(m, StreamMsg::Error(_))));

        let failed = Event::Finished {
            stop: StopReason::EndTurn,
            usage: Usage::default(),
            failed: true,
            summary: "not logged in".into(),
        };
        assert_eq!(
            relay(failed, &mut calls, &asked, &tx),
            Relayed::Finished { failed: true }
        );
        let mut last = None;
        while let Ok(msg) = rx.try_recv() {
            last = Some(msg);
        }
        assert_eq!(
            last,
            Some(StreamMsg::Error(BackendError::ClaudeCode(
                "not logged in".into()
            )))
        );
    }

    /// The stream of a turn whose `claude` printed `lines`, then exited
    /// (`Ok`: its status) or was stopped (`Err`: why), closed as [`turn`]
    /// closes it — the relay and [`close`], without the process or its gate,
    /// which a sandbox may not let a test open (this one's refuses a socket).
    fn closed(lines: &str, exited: Result<&str, &str>, unfed: Option<&str>) -> Vec<StreamMsg> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (named, _) = tokio::sync::watch::channel(std::collections::HashSet::new());
        let mut state = RelayState::default();
        let mut finished = None;
        for line in lines.lines() {
            match take_line(line, &named, &mut state, &|_| true, &tx) {
                Ok(None) => {}
                Ok(Some(failed)) => finished = Some(failed),
                Err(tool) => panic!("{tool} was asked"),
            }
        }
        let exited = exited.map(str::to_string).map_err(str::to_string);
        let close = close(finished, exited, unfed.map(str::to_string));
        if let Some(end) = ending(close, "it said this".into()) {
            let _ = tx.send(end);
        }
        let mut sent = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            sent.push(msg);
        }
        sent
    }

    /// **Every way a turn closes ends its stream once** — and an exit with
    /// no `result` is said, not taken for a clean end: the consumer's turn
    /// gate waits on one ending. Sending `Complete` there passed both crates
    /// (review stop 12, T9 4); and a failed `result`, then a stop or a reap
    /// that failed, ended the turn twice.
    #[test]
    fn every_way_a_turn_closes_ends_its_stream_once() {
        let clean = TURN.trim();
        let (without_result, _) = clean.rsplit_once('\n').expect("a result line");
        let failed = format!(
            "{without_result}\n{}",
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"not logged in","usage":{}}"#
        );
        let code = |why: &str| StreamMsg::Error(BackendError::ClaudeCode(why.into()));
        let exited = StreamMsg::Error(BackendError::ClaudeExited {
            status: "exit status: 1".into(),
            said: "it said this".into(),
        });
        /// The lines `claude` printed, how it ended, the input it was not
        /// sent, and the ending the turn earns.
        type Closing<'a> = (
            &'a str,
            Result<&'a str, &'a str>,
            Option<&'a str>,
            StreamMsg,
        );
        let cases: [Closing; 8] = [
            (clean, Ok("exit status: 0"), None, StreamMsg::Complete),
            (
                clean,
                Ok("exit status: 0"),
                Some("unfed"),
                StreamMsg::Complete,
            ),
            (clean, Err("waiting failed"), None, code("waiting failed")),
            (without_result, Ok("exit status: 1"), None, exited),
            (
                without_result,
                Ok("exit status: 1"),
                Some("unfed"),
                code("unfed"),
            ),
            (without_result, Err("stopped"), None, code("stopped")),
            (&failed, Ok("exit status: 1"), None, code("not logged in")),
            (&failed, Err("waiting failed"), None, code("not logged in")),
        ];
        for (lines, exit, unfed, want) in cases {
            let sent = closed(lines, exit, unfed);
            assert_eq!(the_ending(&sent), &want, "{exit:?} {unfed:?}");
        }
    }

    /// **A call that worked without asking the gate stops the turn** — and is
    /// recorded, since it ran. One that failed unasked does not: Claude Code
    /// answers some calls with an error before any hook runs (review 21,
    /// Reviewer B: the gate was never shown to be in the loop).
    #[test]
    fn a_call_that_ran_unasked_stops_the_turn() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let never = |_: &str| false;
        let called = |id: &str| Event::ToolCalled {
            id: id.into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "ls"}),
            parent: None,
        };
        let returned = |id: &str, is_error| Event::ToolReturned {
            id: id.into(),
            output: "out".into(),
            is_error,
            parent: None,
        };
        let mut calls = RelayState::default();
        assert_eq!(relay(called("t1"), &mut calls, &never, &tx), Relayed::On);
        assert_eq!(
            relay(returned("t1", false), &mut calls, &never, &tx),
            Relayed::Ungated("Bash".into())
        );
        assert!(matches!(rx.try_recv(), Ok(StreamMsg::ToolRecord { .. })));
        // Refused before any hook ran: an error, and the turn goes on.
        relay(called("t2"), &mut calls, &never, &tx);
        assert_eq!(
            relay(returned("t2", true), &mut calls, &never, &tx),
            Relayed::On
        );
        // A success with no call before it, unasked, still stops it.
        assert_eq!(
            relay(returned("t3", false), &mut calls, &never, &tx),
            Relayed::Ungated("t3".into())
        );
        // Asked: the ordinary case.
        let asked = |id: &str| id == "t4";
        relay(called("t4"), &mut calls, &asked, &tx);
        assert_eq!(
            relay(returned("t4", false), &mut calls, &asked, &tx),
            Relayed::On
        );
    }

    /// **`claude` is never handed Shannon's key** — not in the turn, not in
    /// the login check (review 21, Reviewer B: the key reached every Bash
    /// command Claude Code ran) — **nor anything withheld**: a secret's
    /// variable (review stop 14, Reviewer E F1), `[exec] withhold`. The two
    /// spawns are given one environment. And a turn keeps Claude Code's Bash
    /// where Shannon started.
    #[test]
    fn claude_starts_without_shannons_key() {
        let program = Path::new("claude");
        // Built as the backend is, so the scratch directory travels the way
        // a turn takes it (`Launch`).
        let backend = ClaudeCode::new(program, "/bin/shannon", "x").with_tmpdir("/tmp/shannon-501");
        // Withheld — and named in `[exec] env` too, which must not put it
        // back (review stop 14, Reviewer E F1).
        let env = ProviderEnv {
            minimal: false,
            set: vec![("GITHUB_TOKEN".into(), "back".into())],
            withheld: vec!["GITHUB_TOKEN".into()],
        };
        let req = SendRequest {
            env: env.clone(),
            ..SendRequest::new(Vec::new())
        };
        let turn = turn_command(&backend.launch, "x", &req, Path::new("/s"));
        let login = login_command(program, &env);
        for (what, command) in [("login", &login), ("turn", turn.as_std())] {
            let envs: Vec<(String, Option<String>)> = command
                .get_envs()
                .map(|(name, value)| {
                    let text = |s: &std::ffi::OsStr| s.to_string_lossy().into_owned();
                    (text(name), value.map(text))
                })
                .collect();
            // Spelled here, not `withheld_vars()`: a list checked against
            // itself passes with a variable dropped from it.
            for var in crate::config::KEY_VARS.iter().chain([&EFFORT_VAR]) {
                assert!(
                    envs.contains(&(var.to_string(), None)),
                    "{what}: {var} is handed on"
                );
            }
            assert!(
                envs.contains(&("GITHUB_TOKEN".into(), None)),
                "{what}: a withheld variable is handed on: {envs:?}"
            );
            if what == "turn" {
                assert!(envs.contains(&(MAINTAIN_CWD_ENV.into(), Some("1".into()))));
                // Its temporary files in Shannon's scratch directory.
                assert!(envs.contains(&(TMPDIR_ENV.into(), Some("/tmp/shannon-501".into()))));
                for off in [NO_CLAUDE_MDS_ENV, NO_AUTO_MEMORY_ENV] {
                    assert!(
                        !envs.iter().any(|(name, _)| name == off),
                        "{off}: what claude keeps is kept unless a request says not"
                    );
                }
            }
        }
    }

    /// **A request that says not to read CLAUDE.md, or not to keep memories,
    /// runs `claude` without them** — `[memory] claude = "off"` (both) or
    /// `memory = off` (memories), each through the variable Claude Code reads.
    #[test]
    fn a_request_can_turn_claude_mds_and_memory_off() {
        let backend = ClaudeCode::new("claude", "/bin/shannon", "x");
        let set = |req: SendRequest, var: &str| {
            turn_command(&backend.launch, "x", &req, Path::new("/s"))
                .as_std()
                .get_envs()
                .any(|(name, value)| name == var && value == Some("1".as_ref()))
        };
        let req = SendRequest {
            own_instructions: false,
            ..SendRequest::new(Vec::new())
        };
        assert!(set(req.clone(), NO_CLAUDE_MDS_ENV));
        assert!(!set(req, NO_AUTO_MEMORY_ENV), "each its own");
        let req = SendRequest {
            own_memory: false,
            ..SendRequest::new(Vec::new())
        };
        assert!(set(req.clone(), NO_AUTO_MEMORY_ENV));
        assert!(!set(req, NO_CLAUDE_MDS_ENV));
        let req = SendRequest {
            auto_compact: false,
            ..SendRequest::new(Vec::new())
        };
        assert!(set(req.clone(), NO_AUTO_COMPACT_ENV));
        assert!(!set(req, NO_AUTO_MEMORY_ENV));
        assert!(
            !set(SendRequest::new(Vec::new()), NO_AUTO_COMPACT_ENV),
            "unasked, Claude Code compacts as it does"
        );
        let req = SendRequest {
            background_tasks: false,
            ..SendRequest::new(Vec::new())
        };
        assert!(set(req.clone(), NO_BACKGROUND_TASKS_ENV));
        assert!(!set(req, NO_AUTO_COMPACT_ENV));
        assert!(
            !set(SendRequest::new(Vec::new()), NO_BACKGROUND_TASKS_ENV),
            "unasked, Claude Code runs background tasks as it does"
        );
    }

    /// **A request's `[exec] env` reaches `claude`, under Shannon's own** —
    /// the `PATH` its commands run with is yours; its scratch `TMPDIR` is
    /// Shannon's, whatever the env says (Steven, 10-06).
    #[test]
    fn a_requests_env_reaches_claude_under_shannons_own() {
        let backend =
            ClaudeCode::new("claude", "/bin/shannon", "x").with_tmpdir("/tmp/shannon-501");
        let req = SendRequest {
            env: ProviderEnv {
                set: vec![
                    ("PATH".into(), "/opt/bin:/usr/bin".into()),
                    (TMPDIR_ENV.into(), "/elsewhere".into()),
                ],
                ..ProviderEnv::default()
            },
            ..SendRequest::new(Vec::new())
        };
        let turn = turn_command(&backend.launch, "x", &req, Path::new("/s"));
        let value = |var: &str| {
            turn.as_std()
                .get_envs()
                .find(|(name, _)| *name == var)
                .and_then(|(_, value)| value)
                .map(|v| v.to_string_lossy().into_owned())
        };
        assert_eq!(value("PATH").as_deref(), Some("/opt/bin:/usr/bin"));
        assert_eq!(value(TMPDIR_ENV).as_deref(), Some("/tmp/shannon-501"));
    }

    /// **The `claude` a turn runs is found on Shannon's own `PATH`** — the
    /// request's `PATH` is its commands', never the one that picks the
    /// program (review 29, Reviewer B). `sh` stands in: it is on every
    /// `PATH` a test runs under, and on none the request names.
    #[test]
    fn claude_is_found_on_shannons_own_path() {
        let backend = ClaudeCode::new("sh", "/bin/shannon", "x");
        let req = SendRequest {
            env: ProviderEnv {
                set: vec![("PATH".into(), "/nonexistent".into())],
                ..ProviderEnv::default()
            },
            ..SendRequest::new(Vec::new())
        };
        let turn = turn_command(&backend.launch, "x", &req, Path::new("/s"));
        let program = Path::new(turn.as_std().get_program());
        assert!(program.is_absolute(), "{program:?}");
        assert!(
            Path::new(login_command(Path::new("sh"), &ProviderEnv::default()).get_program())
                .is_absolute()
        );
    }

    /// A stand-in `claude` leading its own group, as a turn spawns it.
    fn leading(script: &str) -> crate::exec::Leader {
        let child = tokio::process::Command::new("sh")
            .args(["-c", script])
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .expect("sh");
        crate::exec::Leader::new(child)
    }

    /// Wait, on the runtime, until `done` or a few seconds pass.
    async fn until(done: impl Fn() -> bool) -> bool {
        for _ in 0..150 {
            if done() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        done()
    }

    /// **An ended turn interrupts `claude` first**, so Claude Code can kill
    /// the shells it started in groups of their own — a SIGKILL to its group
    /// never reaches them (review 21, Reviewer B).
    #[tokio::test]
    async fn an_ended_turn_interrupts_claude_first() {
        let dir = tempfile::tempdir().expect("dir");
        let mark = dir.path().join("interrupted");
        let leader = leading(&format!(
            "trap 'touch {}; exit 0' INT; while :; do sleep 0.05; done",
            mark.display()
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        end_gracefully(leader, Duration::from_secs(2));
        assert!(until(|| mark.exists()).await, "it was never interrupted");
    }

    /// **…and kills its group when the interrupt is not answered** — the
    /// group's own processes included.
    #[tokio::test]
    async fn an_interrupt_ignored_is_followed_by_the_group_kill() {
        let dir = tempfile::tempdir().expect("dir");
        let pid_file = dir.path().join("pid");
        let leader = leading(&format!(
            "trap '' INT; sleep 30 & echo $! > {}; wait",
            pid_file.display()
        ));
        let read_pid = || {
            std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|s| s.trim().parse::<i32>().ok())
        };
        assert!(until(|| read_pid().is_some()).await, "no pid written");
        let pid = read_pid().expect("pid");
        // SAFETY: signal 0 only asks whether the process exists.
        let alive = || unsafe { libc::kill(pid, 0) == 0 };
        assert!(alive(), "premise: the probe sees it alive");
        end_gracefully(leader, Duration::from_millis(200));
        assert!(until(|| !alive()).await, "its group outlived the grace");
    }

    /// **The gate's directory is owner-only** — on a shared `/tmp`, the mode
    /// is what keeps another user from replacing the socket (review 21,
    /// Reviewer B: changing it to 0777 passed every test). And two opened at
    /// once are two.
    #[test]
    fn the_gate_directory_is_fresh_and_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let (a, b) = (gate_dir().expect("a"), gate_dir().expect("b"));
        let mode = std::fs::metadata(&a).expect("meta").permissions().mode();
        let _ = (std::fs::remove_dir(&a), std::fs::remove_dir(&b));
        assert_eq!(mode & 0o777, 0o700);
        assert_ne!(a, b);
    }

    /// **Names made in the same instant still differ** — the clock is
    /// microsecond-grained on macOS, and a second turn opened in the same
    /// microsecond refused itself (review 21, Reviewer B). Back to back, far
    /// faster than the clock ticks.
    #[test]
    fn gate_names_made_at_once_differ() {
        let names: std::collections::HashSet<String> = (0..1000).map(|_| unique_suffix()).collect();
        assert_eq!(names.len(), 1000);
    }

    /// **A hook that dies takes its question with it** — withdrawn from the
    /// gate, so no pane waits on an answer nobody will receive. Over a socket
    /// pair, as the hook would hold it: one line written, then gone.
    #[tokio::test]
    async fn a_question_whose_hook_dies_is_withdrawn() {
        let backend = ClaudeCode::new("claude", "/bin/shannon", "claude-code/sonnet");
        let (ours, theirs) = tokio::net::UnixStream::pair().expect("pair");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(question(
            ours,
            backend.pending.clone(),
            Questions::default(),
            tx,
            named(&["toolu_1"]),
        ));
        let (read, mut write) = theirs.into_split();
        write
            .write_all(
                br#"{"call":{"tool_use_id":"toolu_1","tool_name":"Bash","tool_input":{"command":"ls"}}}
"#,
            )
            .await
            .expect("ask");
        let Some(StreamMsg::ToolAsk(ToolQuestion { ask, .. })) = rx.recv().await else {
            panic!("asked");
        };
        assert_eq!(ask.tool_use_id(), "toolu_1");
        drop((read, write));
        task.await.expect("question ends");
        assert_eq!(rx.recv().await, Some(StreamMsg::ToolWithdrawn(ask.clone())));
        assert!(
            !backend.answer_tool(
                &ask,
                Reply::Allow {
                    updated_input: None
                }
            ),
            "nothing is left waiting for an answer"
        );
    }

    /// **A connection that goes takes only its own question** — a second
    /// asker naming a live call, gone, withdraws ITS question; the first stays
    /// waiting and gets its own answer (review of `buffers.md`, 10-15,
    /// Reviewer B F2: by id, the disconnect removed the real entry and sent a
    /// withdrawal for the real decision).
    #[tokio::test]
    async fn a_disconnect_takes_only_its_own_question() {
        let backend = ClaudeCode::new("claude", "/bin/shannon", "claude-code/sonnet");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let line =
            br#"{"call":{"tool_use_id":"toolu_1","tool_name":"Bash","tool_input":{"command":"ls"}}}
"#;
        let connect = || {
            let (ours, theirs) = tokio::net::UnixStream::pair().expect("pair");
            tokio::spawn(question(
                ours,
                backend.pending.clone(),
                Arc::default(),
                tx.clone(),
                named(&["toolu_1"]),
            ));
            theirs.into_split()
        };
        let (real_read, mut real_write) = connect();
        real_write.write_all(line).await.expect("ask");
        let Some(StreamMsg::ToolAsk(ToolQuestion { ask: real, .. })) = rx.recv().await else {
            panic!("asked");
        };
        let (spoof_read, mut spoof_write) = connect();
        spoof_write.write_all(line).await.expect("ask again");
        let Some(StreamMsg::ToolAsk(ToolQuestion { ask: spoof, .. })) = rx.recv().await else {
            panic!("asked again");
        };
        drop((spoof_read, spoof_write));
        assert_eq!(rx.recv().await, Some(StreamMsg::ToolWithdrawn(spoof)));
        assert_eq!(
            lock(&backend.pending).len(),
            1,
            "the second's entry went with it (Reviewer E 7); the first's waits"
        );
        assert!(
            backend.answer_tool(
                &real,
                Reply::Allow {
                    updated_input: None
                }
            ),
            "the first is still waiting"
        );
        let mut answer = String::new();
        tokio::time::timeout(
            Duration::from_secs(5),
            BufReader::new(real_read).read_line(&mut answer),
        )
        .await
        .expect("answered, not left waiting")
        .expect("read");
        assert!(answer.contains("allow"), "{answer}");
        drop(real_write);
    }

    /// **A connection that never asks, or never stops asking, is refused** —
    /// any local process can reach the socket, so neither may hold a task or
    /// memory (review 21, Reviewer B).
    #[tokio::test]
    async fn a_question_is_bounded_in_time_and_size() {
        async fn answer_to(sent: &[u8]) -> String {
            let backend = ClaudeCode::new("claude", "/bin/shannon", "claude-code/sonnet");
            let (ours, theirs) = tokio::net::UnixStream::pair().expect("pair");
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(question(
                ours,
                backend.pending.clone(),
                Arc::default(),
                tx,
                named(&[]),
            ));
            let (read, mut write) = theirs.into_split();
            write.write_all(sent).await.expect("send");
            // Held open: neither a newline nor an end arrives.
            let mut answer = String::new();
            tokio::time::timeout(
                Duration::from_secs(5),
                BufReader::new(read).read_line(&mut answer),
            )
            .await
            .expect("answered, not left waiting")
            .expect("read");
            drop(write);
            answer
        }
        let silent = answer_to(b"").await;
        assert!(silent.contains("did not arrive in time"), "{silent}");
        let endless = answer_to(&vec![b'x'; QUESTION_CAP as usize + 10]).await;
        assert!(endless.contains("did not parse"), "{endless}");
    }

    /// A turn's named calls, `ids` named already — its sender gone, as at a
    /// turn's end, which keeps what was named.
    fn named(ids: &[&str]) -> Named {
        let (_, named) = tokio::sync::watch::channel(ids.iter().map(|id| id.to_string()).collect());
        named
    }

    /// A question about `id`, as the hook writes it.
    fn asking(id: &str) -> String {
        format!(
            "{{\"call\":{{\"tool_use_id\":\"{id}\",\"tool_name\":\"Bash\",\"tool_input\":{{\"command\":\"ls\"}}}}}}\n"
        )
    }

    /// What the hook reads back, waited for within a bound.
    async fn answered(read: tokio::net::unix::OwnedReadHalf) -> String {
        let mut answer = String::new();
        tokio::time::timeout(
            Duration::from_secs(5),
            BufReader::new(read).read_line(&mut answer),
        )
        .await
        .expect("answered, not left waiting")
        .expect("read");
        answer
    }

    /// **A question about a call the turn's output never named is refused,
    /// unasked** (B F2, property 4) — any shell Claude Code starts can reach
    /// the socket, so one could ask about a call it made up. Never put to
    /// the pane; said, as [`StreamMsg::ToolUnnamed`]; and Claude Code's
    /// model is told what to do instead. Its turn still running: the bound
    /// is what ends the wait.
    #[tokio::test]
    async fn a_question_about_a_call_never_named_is_refused_unasked() {
        let backend = ClaudeCode::new("claude", "/bin/shannon", "claude-code/sonnet");
        let (names, named) = tokio::sync::watch::channel(std::collections::HashSet::new());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (ours, theirs) = tokio::net::UnixStream::pair().expect("pair");
        let started = std::time::Instant::now();
        tokio::spawn(question(
            ours,
            backend.pending.clone(),
            Questions::default(),
            tx,
            named,
        ));
        let (read, mut write) = theirs.into_split();
        write
            .write_all(asking("toolu_made_up").as_bytes())
            .await
            .expect("ask");
        let answer = answered(read).await;
        assert!(answer.contains("not in Claude Code's output"), "{answer}");
        assert!(started.elapsed() >= NAMED_WITHIN, "premise: it waited");
        assert_eq!(
            rx.recv().await,
            Some(StreamMsg::ToolUnnamed {
                id: "toolu_made_up".into(),
                name: "Bash".into()
            })
        );
        assert!(rx.try_recv().is_err(), "never put to the gate");
        assert!(lock(&backend.pending).is_empty(), "nothing waits on it");
        drop(names);
    }

    /// **A call named while its question waits is asked** — whatever order
    /// Claude Code writes a call and starts its hook in (undocumented), the
    /// call's line can reach Shannon after the question: one bigger than
    /// the pipe holds arrives in pieces. Not before it is named.
    #[tokio::test]
    async fn a_call_named_while_its_question_waits_is_asked() {
        let backend = ClaudeCode::new("claude", "/bin/shannon", "claude-code/sonnet");
        let (names, named) = tokio::sync::watch::channel(std::collections::HashSet::new());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (ours, theirs) = tokio::net::UnixStream::pair().expect("pair");
        tokio::spawn(question(
            ours,
            backend.pending.clone(),
            Questions::default(),
            tx,
            named,
        ));
        let (read, mut write) = theirs.into_split();
        write
            .write_all(asking("toolu_late").as_bytes())
            .await
            .expect("ask");
        tokio::time::sleep(NAMED_WITHIN / 3).await;
        assert!(rx.try_recv().is_err(), "not asked before it is named");
        names.send_modify(|ids| {
            ids.insert("toolu_late".to_string());
        });
        let asked = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        let Ok(Some(StreamMsg::ToolAsk(ToolQuestion { ask, answer_by, .. }))) = asked else {
            panic!("asked once named: {asked:?}");
        };
        assert_eq!(ask.tool_use_id(), "toolu_late");
        // **The hook's clock, not the gate's**: its deadline ran while the
        // question waited to be named, so what is left is less by that wait.
        let waited = NAMED_WITHIN / 3;
        let left = answer_by.map(|at| at.saturating_duration_since(std::time::Instant::now()));
        assert!(
            left.is_some_and(|w| w <= HOOK_DEADLINE - waited),
            "less the wait to be named: {left:?}"
        );
        let allow = Reply::Allow {
            updated_input: None,
        };
        assert!(backend.answer_tool(&ask, allow));
        assert!(answered(read).await.contains("allow"));
        drop(write);
    }

    /// **A line's calls are named to the gate as it is taken in** — a call
    /// whole (an `assistant` line's `tool_use`, a subagent's too) and a
    /// top-level call as its block begins (`content_block_start`, which
    /// records nothing). The loop's work for a line (`take_line`); the
    /// socket the rest of a turn needs cannot be bound where this suite
    /// usually runs.
    #[test]
    fn a_lines_calls_are_named_to_the_gate() {
        let (names, named) = tokio::sync::watch::channel(std::collections::HashSet::new());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = RelayState::default();
        let mut take = |line: &str| take_line(line, &names, &mut state, &|_| false, &tx);
        let begun = r#"{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_3","name":"Write","input":{}}}}"#;
        assert_eq!(take(begun), Ok(None));
        assert!(rx.try_recv().is_err(), "a call begun records nothing");
        for line in [
            r#"{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{}}]}}"#,
            r#"{"type":"assistant","parent_tool_use_id":"toolu_task","message":{"content":[{"type":"tool_use","id":"toolu_2","name":"Bash","input":{}}]}}"#,
            r#"{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"toolu_9"}}}"#,
        ] {
            assert_eq!(take(line), Ok(None));
        }
        let mut seen: Vec<String> = named.borrow().iter().cloned().collect();
        seen.sort();
        assert_eq!(seen, ["toolu_1", "toolu_2", "toolu_3"]);
    }

    /// **A question reaches the gate, and its answer the hook — once.** And
    /// every way of not being answered refuses: the turn gone before the
    /// question, the turn ending with it open.
    #[tokio::test]
    async fn a_question_is_answered_once_and_otherwise_refused() {
        let backend = ClaudeCode::new("claude", "/bin/shannon", "claude-code/sonnet");
        let ids = Questions::default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        // Every wait is bounded: a refusal that is never sent would otherwise
        // show as a hang, not a failure (review 21, Reviewer B).
        async fn soon<T>(what: impl std::future::Future<Output = T>) -> T {
            tokio::time::timeout(Duration::from_secs(5), what)
                .await
                .expect("answered, not left waiting")
        }
        let call = serde_json::json!({
            "tool_use_id": "toolu_9", "tool_name": "Bash", "tool_input": {"command": "ls"},
            "cwd": "/work"
        });
        // Asked a minute ago, by the hook's clock.
        let a_minute = Duration::from_secs(60);
        let asked_at = std::time::Instant::now()
            .checked_sub(a_minute)
            .expect("the clock reaches a minute back");
        let waiting = tokio::spawn({
            let (pending, ids, tx) = (backend.pending.clone(), ids.clone(), tx.clone());
            async move { ask(call.clone(), asked_of(&call), asked_at, &pending, &ids, &tx).await }
        });
        let Some(StreamMsg::ToolAsk(ToolQuestion {
            id,
            name,
            input,
            cwd,
            ask: asked,
            answer_by,
        })) = rx.recv().await
        else {
            panic!("asked");
        };
        assert_eq!(
            (id.as_str(), name.as_str(), input, cwd.as_deref()),
            (
                "toolu_9",
                "Bash",
                serde_json::json!({"command": "ls"}),
                Some("/work")
            )
        );
        assert_eq!(asked.tool_use_id(), "toolu_9");
        // **The hook's deadline, said**: where it was asked, by the hook's
        // clock, and on — whatever the consumer was doing meanwhile.
        assert_eq!(
            answer_by,
            asked_at.checked_add(HOOK_DEADLINE),
            "a hook's question has its deadline"
        );
        // Another question's serial, though the same call's id: not this one.
        let other = AskId::for_test("toolu_9", asked.serial() + 1_000_000);
        assert!(!backend.answer_tool(&other, Reply::Deny { reason: "x".into() }));
        assert!(backend.answer_tool(
            &asked,
            Reply::Allow {
                updated_input: None
            }
        ));
        assert!(
            !backend.answer_tool(
                &asked,
                Reply::Allow {
                    updated_input: None
                }
            ),
            "answered once"
        );
        assert_eq!(
            soon(waiting).await.expect("join"),
            Reply::Allow {
                updated_input: None
            }
        );

        // The turn ends with a question open: refused.
        let asked = Asked {
            pending: backend.pending.clone(),
            ids: ids.clone(),
        };
        let open = tokio::spawn({
            let (pending, ids, tx) = (backend.pending.clone(), ids.clone(), tx.clone());
            let call = serde_json::json!({"tool_use_id": "toolu_10", "tool_name": "Read"});
            let now = std::time::Instant::now();
            async move { ask(call.clone(), asked_of(&call), now, &pending, &ids, &tx).await }
        });
        soon(rx.recv()).await.expect("asked");
        drop(asked);
        assert!(matches!(
            soon(open).await.expect("join"),
            Reply::Deny { .. }
        ));

        // A question for a turn that has ended: refused, and nothing waits
        // in the map (review stop 7, Reviewer E 6).
        let call = serde_json::json!({"tool_use_id": "toolu_12", "tool_name": "Bash"});
        assert_eq!(
            soon(ask(
                call.clone(),
                asked_of(&call),
                std::time::Instant::now(),
                &backend.pending,
                &ids,
                &tx
            ))
            .await,
            Reply::Deny {
                reason: "the turn ended before Shannon answered".into()
            }
        );
        assert!(lock(&backend.pending).is_empty(), "no entry left behind");

        // Nobody left to ask: refused at once, and its entry taken back
        // (Reviewer E 7) — a turn still running, so it is the send that fails.
        let ids = Questions::default();
        drop(rx);
        let call = serde_json::json!({"tool_use_id": "toolu_11", "tool_name": "Bash"});
        assert_eq!(
            soon(ask(
                call.clone(),
                asked_of(&call),
                std::time::Instant::now(),
                &backend.pending,
                &ids,
                &tx
            ))
            .await,
            Reply::Deny {
                reason: "nobody is there to decide (Shannon's turn is gone)".into()
            }
        );
        assert!(lock(&backend.pending).is_empty(), "no entry left behind");
        let nameless = serde_json::json!({"tool_name": "Bash"});
        assert!(matches!(
            soon(ask(
                nameless.clone(),
                asked_of(&nameless),
                std::time::Instant::now(),
                &backend.pending,
                &ids,
                &tx
            ))
            .await,
            Reply::Deny { .. }
        ));
    }

    /// **Two questions naming one call are two questions** (review of
    /// `buffers.md`, 10-15, Reviewer B F2): a second asker — any process
    /// holding the socket can be one — gets its own serial, an answer to it
    /// reaches it alone, and the first stays waiting for its own; and a turn
    /// that ends takes its own questions, not another turn's naming the same
    /// call. Keyed by the asker's id, the second replaced the first's way
    /// back, and either ending removed both.
    #[tokio::test]
    async fn a_second_question_for_one_call_is_answered_apart() {
        let backend = ClaudeCode::new("claude", "/bin/shannon", "claude-code/sonnet");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        async fn soon<T>(what: impl std::future::Future<Output = T>) -> T {
            tokio::time::timeout(Duration::from_secs(5), what)
                .await
                .expect("answered, not left waiting")
        }
        let (mine, theirs): (Questions, Questions) = (Arc::default(), Arc::default());
        let asking = |input: &str, ids: &Questions| {
            let call = serde_json::json!({
                "tool_use_id": "toolu_7", "tool_name": "Bash", "tool_input": {"command": input}
            });
            let (pending, ids, tx) = (backend.pending.clone(), ids.clone(), tx.clone());
            let now = std::time::Instant::now();
            tokio::spawn(async move {
                ask(call.clone(), asked_of(&call), now, &pending, &ids, &tx).await
            })
        };
        let real = asking("rm -rf build", &mine);
        let Some(StreamMsg::ToolAsk(ToolQuestion { ask: first, .. })) = soon(rx.recv()).await
        else {
            panic!("asked");
        };
        let spoof = asking("ls", &theirs);
        let Some(StreamMsg::ToolAsk(ToolQuestion { ask: second, .. })) = soon(rx.recv()).await
        else {
            panic!("asked again");
        };
        assert_ne!(first, second, "two serials for one call's id");

        // The second asker's turn ends: its question goes, the first stays.
        drop(Asked {
            pending: backend.pending.clone(),
            ids: theirs,
        });
        assert!(matches!(
            soon(spoof).await.expect("join"),
            Reply::Deny { .. }
        ));
        // An answer for the first reaches the first, and only it.
        assert!(backend.answer_tool(
            &first,
            Reply::Deny {
                reason: "no".into()
            }
        ));
        assert_eq!(
            soon(real).await.expect("join"),
            Reply::Deny {
                reason: "no".into()
            }
        );
        assert!(
            !backend.answer_tool(
                &second,
                Reply::Allow {
                    updated_input: None
                }
            ),
            "nothing waits under the second serial"
        );
        drop(mine);
    }

    /// What a stand-in `claude` printing `turn` came to: whether this machine
    /// can bind a socket (asked first, then required of the outcome), what the
    /// turn sent, and the file the stand-in wrote its arguments to — present
    /// only if it ran. The stand-in runs no hook: it cannot ask the gate.
    async fn stand_in_turn(turn: &str) -> (bool, Vec<StreamMsg>, Option<String>) {
        let can_bind = {
            let probe = tempfile::tempdir().expect("dir");
            std::os::unix::net::UnixListener::bind(probe.path().join("probe.sock")).is_ok()
        };
        let dir = tempfile::tempdir().expect("dir");
        let ran = dir.path().join("ran");
        let script = dir.path().join("claude");
        let events = turn.trim().replace('\'', "'\\''");
        // `ran` records the arguments too: the request's effort reaching the
        // `claude` that runs is held by nothing else (Reviewer, 10-04).
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat > /dev/null\nprintf '%s\\n' '{events}'\n",
                ran.display()
            ),
        )
        .expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
                .expect("chmod");
        }
        let backend = ClaudeCode::new(&script, "/bin/shannon", "claude-code/sonnet");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let req = SendRequest::new(vec![Message::user("hi")]).with_effort(Some(crate::Effort::Max));
        backend.send_message(req, tx).await.expect("turn");
        let mut sent = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            sent.push(msg);
        }
        (can_bind, sent, std::fs::read_to_string(&ran).ok())
    }

    /// Where no socket can be bound (the sandbox this suite is often run in
    /// refuses one), the turn refuses to start: no gate, no turn, and the
    /// stand-in never ran. True of `sent` there, whatever the turn was.
    fn refused_without_a_gate(sent: &[StreamMsg], ran: Option<&String>) {
        let [StreamMsg::Error(BackendError::ClaudeCode(why))] = sent else {
            panic!("premise: no socket, so the turn refuses: {sent:?}");
        };
        assert!(why.contains("gate") && why.contains("nothing ran"), "{why}");
        assert!(ran.is_none(), "claude ran without its gate");
    }

    /// **A turn end to end, with a stand-in `claude`.** Where a socket can be
    /// opened, its stream arrives as Shannon's, the request's effort reaching
    /// the `claude` that runs; where it cannot, the turn refuses to start
    /// (`refused_without_a_gate`). Which one is asked of the machine first,
    /// then required: a gate broken everywhere must not pass as "this machine
    /// cannot bind".
    ///
    /// The turn makes NO call. A stand-in runs no hook, so a call it reported
    /// would be one that bypassed the gate, which stops the turn (review 21;
    /// the next test). Until Linux ran this suite (11-05), the turn here had
    /// one — and every run of it on this Mac was inside a sandbox that cannot
    /// bind, so this branch had not run since that stop went in.
    #[tokio::test]
    async fn a_turn_runs_under_its_gate_or_not_at_all() {
        let no_call: String = TURN
            .lines()
            .filter(|line| !line.contains("toolu_1"))
            .collect::<Vec<_>>()
            .join("\n");
        let (can_bind, sent, ran) = stand_in_turn(&no_call).await;
        if !can_bind {
            return refused_without_a_gate(&sent, ran.as_ref());
        }
        let args = ran.unwrap_or_else(|| panic!("a socket bound, so claude ran: {sent:?}"));
        let args: Vec<&str> = args.lines().collect();
        let at = args.iter().position(|a| *a == "--effort");
        assert_eq!(
            at.and_then(|i| args.get(i + 1)),
            Some(&"max"),
            "the request's effort is the claude's: {args:?}"
        );
        assert_eq!(
            sent.first(),
            Some(&StreamMsg::ProviderSession("sess-1".into()))
        );
        assert_eq!(sent.last(), Some(&StreamMsg::Complete), "{sent:?}");
    }

    /// **A call the gate never heard of stops the turn, end to end** — the
    /// stand-in reports a Bash call and its successful result, and no
    /// question ever came: as a `disableAllHooks` would make it (review 21,
    /// Reviewer B). The turn is stopped, saying why, and never completes.
    #[tokio::test]
    async fn a_call_the_gate_never_heard_of_stops_the_turn() {
        let (can_bind, sent, ran) = stand_in_turn(TURN).await;
        if !can_bind {
            return refused_without_a_gate(&sent, ran.as_ref());
        }
        assert!(ran.is_some(), "premise: claude ran: {sent:?}");
        let Some(StreamMsg::Error(BackendError::ClaudeCode(why))) = sent.last() else {
            panic!("the turn was not stopped: {sent:?}");
        };
        assert!(why.contains("ran without passing Shannon's gate"), "{why}");
        assert!(!sent.contains(&StreamMsg::Complete), "{sent:?}");
    }

    /// **Each file tool's write, as the pane shows it**: the file and what
    /// lands in it, for exactly the tools read as `cat >`.
    #[test]
    fn a_file_tool_says_what_it_writes() {
        use serde_json::json;
        assert_eq!(
            change("Write", &json!({"file_path": "/a", "content": "x"})),
            Some(("/a".into(), Change::Content("x".into())))
        );
        assert_eq!(
            change(
                "Edit",
                &json!({"file_path": "/a", "old_string": "o", "new_string": "n"})
            ),
            Some(("/a".into(), Change::Edits(vec![("o".into(), "n".into())])))
        );
        let multi = json!({"file_path": "/a", "edits": [
            {"old_string": "o1", "new_string": "n1"}, {"old_string": "o2", "new_string": "n2"}]});
        assert_eq!(
            change("MultiEdit", &multi),
            Some((
                "/a".into(),
                Change::Edits(vec![("o1".into(), "n1".into()), ("o2".into(), "n2".into())])
            ))
        );
        assert_eq!(
            change(
                "NotebookEdit",
                &json!({"notebook_path": "/n.ipynb", "new_source": "s"})
            ),
            Some(("/n.ipynb".into(), Change::Content("s".into())))
        );
        // Missing what it needs, or not a file tool: nothing to show.
        assert_eq!(change("Write", &json!({"file_path": "/a"})), None);
        assert_eq!(
            change(
                "MultiEdit",
                &json!({"file_path": "/a", "edits": [{"old_string": "o"}]})
            ),
            None
        );
        assert_eq!(change("Bash", &json!({"command": "ls"})), None);
        assert_eq!(change("Read", &json!({"file_path": "/a"})), None);
    }

    /// **A `Bash` call asking to leave Claude Code's sandbox asks every
    /// time** (the 11-07 review, finding 17): its line is the command, read
    /// and shown as ever, and its judge is no rule's. `false` or absent, the
    /// rules decide; anything else fails closed — a value that is not a
    /// boolean is not "no".
    #[test]
    fn a_call_leaving_the_sandbox_is_no_rules_to_allow() {
        use serde_json::json;
        let read =
            |input: Value| reading("Bash", &input, WebPolicy::Ask, &|_| false, &|_| Ok(None));
        for leaves in [json!(true), json!("true"), json!(1)] {
            let r = read(json!({"command": "ls", "dangerouslyDisableSandbox": leaves}));
            assert_eq!(r.judge, Judge::Unsandboxed, "{leaves}");
            assert_eq!(r.line, "ls", "the command is still what is shown");
        }
        for stays in [
            json!({"command": "ls", "dangerouslyDisableSandbox": false}),
            json!({"command": "ls", "dangerouslyDisableSandbox": null}),
            json!({"command": "ls"}),
        ] {
            assert_eq!(read(stays.clone()).judge, Judge::Rules, "{stays}");
        }
    }

    /// **Each tool reads as the shell line with its effect**; what the rules
    /// cannot read asks, and only `TodoWrite` and an allowed web go unasked.
    #[test]
    fn tools_read_as_lines_with_their_effect() {
        use serde_json::json;
        let is_file = |path: &str| path == "/src/main.rs";
        let here = |_: &str| Ok(None);
        let read =
            |name: &str, input: Value| reading(name, &input, WebPolicy::Ask, &is_file, &here);
        let rules = |line: &str| Reading {
            line: line.into(),
            judge: Judge::Rules,
        };
        assert_eq!(read("Bash", json!({"command": "ls -la"})), rules("ls -la"));
        assert_eq!(
            read("Read", json!({"file_path": "/a b/it's"})),
            rules(r"cat '/a b/it'\''s'")
        );
        assert_eq!(
            read("Write", json!({"file_path": "/x"})),
            rules("cat > '/x'")
        );
        // An edit reads its file before it writes it (`files::reading`).
        for tool in ["Edit", "MultiEdit"] {
            assert_eq!(
                read(tool, json!({"file_path": "/x"})),
                rules("cat '/x' > '/x'")
            );
        }
        assert_eq!(
            read("NotebookEdit", json!({"notebook_path": "/n.ipynb"})),
            rules("cat > '/n.ipynb'")
        );
        assert_eq!(
            read("Grep", json!({"pattern": "-v x"})),
            rules("grep -r -e '-v x' '.'")
        );
        assert_eq!(
            read("Grep", json!({"pattern": "k", "path": "/src"})),
            rules("grep -r -e 'k' '/src'")
        );
        // A file is read, not searched as a tree: no `-r`, so it is judged
        // as the read it is.
        assert_eq!(
            read("Grep", json!({"pattern": "k", "path": "/src/main.rs"})),
            rules("grep -e 'k' '/src/main.rs'")
        );
        assert_eq!(
            read("Glob", json!({"pattern": "**/*.rs"})),
            rules("find '.'")
        );

        for (name, input) in [
            ("WebFetch", json!({"url": "https://x"})),
            ("WebSearch", json!({"query": "q"})),
            ("Task", json!({"description": "d"})),
            ("mcp__server__tool", json!({})),
            ("Bash", json!({})),
            ("Read", json!({"file_path": ""})),
            ("Grep", json!({})),
            ("SomethingNew", json!({"a": 1})),
        ] {
            assert_eq!(
                read(name, input.clone()).judge,
                Judge::Ask,
                "{name} {input}"
            );
        }
        assert_eq!(
            read("WebFetch", json!({"url": "https://x"})).line,
            "WebFetch https://x"
        );
        assert_eq!(read("Task", json!({"description": "d"})).line, "Task d");
        assert_eq!(read("TodoWrite", json!({"todos": []})).judge, Judge::Inert);
        let allowed = |name: &str| {
            reading(
                name,
                &json!({"url": "u"}),
                WebPolicy::Allow,
                &is_file,
                &here,
            )
            .judge
        };
        assert_eq!(allowed("WebFetch"), Judge::Allow);
        assert_eq!(allowed("WebSearch"), Judge::Allow);
        assert_eq!(
            allowed("Task"),
            Judge::Ask,
            "the web knob is the web's alone"
        );

        // A long input is cut on a character, not a byte.
        let long = read("mcp__x", json!({"text": "é".repeat(500)}));
        assert!(
            long.line.ends_with('…') && long.line.chars().count() < 220,
            "{}",
            long.line
        );
    }

    /// **A file a tool opens through a link is judged where it lands too**
    /// (F2c) — each file tool, a notebook, a Grep of a file — and one whose
    /// landing cannot be named to the rules asks.
    #[test]
    fn a_file_through_a_link_is_judged_where_it_lands() {
        use serde_json::json;
        let is_file = |path: &str| path == "/p/n";
        let lands = |path: &str| match path {
            "/p/n" => Ok(Some("/q/s".to_string())),
            "/p/odd" => Err("not text".to_string()),
            _ => Ok(None),
        };
        let read =
            |name: &str, input: Value| reading(name, &input, WebPolicy::Ask, &is_file, &lands);
        for (name, input, line) in [
            (
                "Read",
                json!({"file_path": "/p/n"}),
                "cat '/p/n'; cat '/q/s'",
            ),
            (
                "Write",
                json!({"file_path": "/p/n"}),
                "cat > '/p/n'; cat > '/q/s'",
            ),
            (
                "Edit",
                json!({"file_path": "/p/n"}),
                "cat '/p/n' > '/p/n'; cat '/q/s' > '/q/s'",
            ),
            (
                "NotebookEdit",
                json!({"notebook_path": "/p/n"}),
                "cat > '/p/n'; cat > '/q/s'",
            ),
            (
                "Grep",
                json!({"pattern": "k", "path": "/p/n"}),
                "grep -e 'k' '/p/n'; grep -e 'k' '/q/s'",
            ),
        ] {
            let read = read(name, input);
            assert_eq!(
                (read.line.as_str(), read.judge),
                (line, Judge::Rules),
                "{name}"
            );
        }
        let odd = read("Read", json!({"file_path": "/p/odd"}));
        assert_eq!(odd.judge, Judge::Ask, "{}", odd.line);
    }

    /// **Claude Code's write to a file of two names says its reach, as
    /// Shannon's own does** (review stop 16, E 3: `files::reach`, the one
    /// rule both doors consult) — a Write, an Edit, a notebook; a Read and a
    /// Grep of it do not.
    #[test]
    fn claude_codes_write_to_a_file_of_two_names_says_its_reach() {
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("theirs"), "x\n").unwrap();
        let rc = dir.path().join("rc");
        std::fs::hard_link(dir.path().join("theirs"), &rc).unwrap();
        let rc = rc.display().to_string();
        let lands = |_: &str| Ok(None);
        let is_file = |_: &str| true;
        let read =
            |name: &str, input: Value| reading(name, &input, WebPolicy::Ask, &is_file, &lands).line;
        let reach = format!("cat > $(find / -samefile '{rc}')");
        assert_eq!(
            read("Write", json!({"file_path": rc})),
            format!("cat > '{rc}'; {reach}")
        );
        assert_eq!(
            read("Edit", json!({"file_path": rc})),
            format!("cat '{rc}' > '{rc}'; {reach}")
        );
        assert_eq!(
            read("NotebookEdit", json!({"notebook_path": rc})),
            format!("cat > '{rc}'; {reach}")
        );
        assert_eq!(
            read("Read", json!({"file_path": rc})),
            format!("cat '{rc}'")
        );
        assert_eq!(
            read("Grep", json!({"pattern": "k", "path": rc})),
            format!("grep -e 'k' '{rc}'")
        );
    }

    /// **An edit maps back onto the input where one field holds it**: `Bash`'s
    /// command, a `Write`'s content, an `Edit`'s new text — what `e` opens is
    /// what it replaces, the rest of the input kept. Any other tool's line is
    /// Shannon's reading of it, and a `MultiEdit`'s new texts are several.
    #[test]
    fn an_edit_maps_back_where_one_field_holds_it() {
        use serde_json::json;
        let input = json!({"command": "rm -rf /", "description": "d"});
        assert_eq!(edit_text("Bash", &input).as_deref(), Some("rm -rf /"));
        assert_eq!(
            edited_input("Bash", &input, "ls"),
            Some(json!({"command": "ls", "description": "d"}))
        );
        let write = json!({"file_path": "/x.rs", "content": "old"});
        assert_eq!(edit_text("Write", &write).as_deref(), Some("old"));
        assert_eq!(
            edited_input("Write", &write, "new"),
            Some(json!({"file_path": "/x.rs", "content": "new"}))
        );
        let edit = json!({"file_path": "/x.rs", "old_string": "a", "new_string": "b"});
        assert_eq!(edit_text("Edit", &edit).as_deref(), Some("b"));
        assert_eq!(
            edited_input("Edit", &edit, "c"),
            Some(json!({"file_path": "/x.rs", "old_string": "a", "new_string": "c"}))
        );
        for (tool, input) in [
            ("Read", json!({"file_path": "/x"})),
            ("MultiEdit", json!({"file_path": "/x", "edits": []})),
        ] {
            assert_eq!(edit_text(tool, &input), None, "{tool}");
            assert_eq!(edited_input(tool, &input, "cat /y"), None, "{tool}");
        }
    }

    /// A stand-in `claude` that runs `body` as `sh`.
    fn fake_claude(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("claude");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        }
        path
    }

    /// **The login check reads Claude Code's own answer**, and only the two
    /// fields it needs: the email in the same JSON never reaches the status.
    /// An old Claude Code with no `auth status` is "could not tell", not "log
    /// in" — that would send the user to fix the wrong thing.
    #[test]
    fn the_login_check_reads_claude_codes_answer_and_nothing_else() {
        let dir = tempfile::tempdir().expect("dir");
        let check = |body: &str| {
            let sub = tempfile::tempdir_in(dir.path()).expect("dir");
            login_status(
                &fake_claude(sub.path(), body),
                &ProviderEnv::default(),
                Duration::from_secs(5),
            )
        };
        let yes = check(
            r#"echo '{"loggedIn": true, "authMethod": "claude.ai", "email": "me@example.com"}'"#,
        );
        assert_eq!(yes, LoginStatus::LoggedIn("claude.ai".into()));
        assert!(!format!("{yes:?}").contains("example.com"));
        assert_eq!(
            check(r#"echo '{"loggedIn": false, "authMethod": "none"}'; exit 1"#),
            LoginStatus::NotLoggedIn
        );
        assert!(matches!(
            check("echo 'error: unknown command auth' >&2; exit 1"),
            LoginStatus::Unknown(_)
        ));
        assert_eq!(
            login_status(
                &dir.path().join("no-such-claude"),
                &ProviderEnv::default(),
                Duration::from_secs(1)
            ),
            LoginStatus::NotInstalled
        );
    }

    /// **A check that does not answer is cut off, and says so** — it runs on
    /// the UI's thread, so its bound is the bound on a frozen screen.
    ///
    /// The margins are wide on both sides: a child that would live 30 s
    /// against a bound of 10 s. They were 5 s and 3 s, and a deadline too
    /// short for a loaded suite to start the shell (docketed at review stop
    /// 2) — a test that fails without a bug teaches the next reader to rerun
    /// rather than read.
    /// **`claude` is given the environment `[claude_code] env` says**, as a
    /// process sees it (a `get_envs` cannot show an `env_clear`): `inherit`
    /// hands on what Shannon holds; `minimal` only what Shannon's own
    /// commands get — `HOME`, not a variable outside that set; and a
    /// withheld name is gone either way (Steven, 10-31, S18-Q1).
    #[test]
    fn claude_is_given_the_environment_its_policy_says() {
        assert!(
            std::env::var_os("CARGO_PKG_NAME").is_some() && std::env::var_os("HOME").is_some(),
            "premise: cargo's variable and a home are in this process's environment"
        );
        let dir = tempfile::tempdir().expect("dir");
        let claude = fake_claude(
            dir.path(),
            "a=${CARGO_PKG_NAME:+inherited}; h=${HOME:+home}\n\
             printf '{\"loggedIn\": true, \"authMethod\": \"%s %s\"}' \"${a:-clean}\" \"${h:-nohome}\"",
        );
        let said = |minimal: bool, withheld: &[&str]| {
            let env = ProviderEnv {
                minimal,
                set: Vec::new(),
                withheld: withheld.iter().map(|w| w.to_string()).collect(),
            };
            login_status(&claude, &env, Duration::from_secs(5))
        };
        let logged = |how: &str| LoginStatus::LoggedIn(how.to_string());
        assert_eq!(said(false, &[]), logged("inherited home"));
        assert_eq!(said(true, &[]), logged("clean home"));
        assert_eq!(said(true, &["HOME"]), logged("clean nohome"));
        assert_eq!(said(false, &["CARGO_PKG_NAME"]), logged("clean home"));
    }

    #[test]
    fn a_login_check_that_hangs_is_cut_off() {
        let dir = tempfile::tempdir().expect("dir");
        let started = std::time::Instant::now();
        let env = ProviderEnv::default();
        let status = login_status(
            &fake_claude(dir.path(), "sleep 30"),
            &env,
            Duration::from_millis(200),
        );
        assert!(matches!(status, LoginStatus::Unknown(_)), "{status:?}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "bounded by the timeout, not by the child: {:?}",
            started.elapsed()
        );

        // One that exits at once, leaving a grandchild holding its output
        // open: its output was read after the exit, so this held the UI for
        // the grandchild's lifetime (review 21, Reviewer B). Its deadline
        // gives the shell time to start and say the grandchild's pid: at
        // 200 ms, under a loaded suite, the group was killed before it had,
        // and the test failed on the missing pid, not on the code.
        let pid_file = dir.path().join("grandchild");
        let started = std::time::Instant::now();
        let status = login_status(
            &fake_claude(
                dir.path(),
                &format!("sleep 30 & echo $! > {}; exit 0", pid_file.display()),
            ),
            &env,
            Duration::from_secs(3),
        );
        assert!(matches!(status, LoginStatus::Unknown(_)), "{status:?}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "bounded by the timeout, not by the grandchild: {:?}",
            started.elapsed()
        );
        // …and the grandchild is ended with it: its group is killed.
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .expect("pid")
            .trim()
            .parse()
            .expect("a pid");
        // SAFETY: signal 0 only asks whether the process exists.
        let alive = |pid: i32| unsafe { libc::kill(pid, 0) == 0 };
        assert!(alive(std::process::id() as i32), "premise: the probe sees");
        let gone = (0..250).any(|_| {
            std::thread::sleep(Duration::from_millis(20));
            !alive(pid)
        });
        assert!(gone, "the grandchild outlived the check");

        // One that closes its output and goes on running: the reading ends,
        // and the wait for the exit is what the deadline must bound — left
        // unbounded, all 36 of this module's tests passed (review stop 4,
        // A2 4). The deadline gives the shell time to close its output, so
        // it is the wait, not the reading, that it cuts off.
        let started = std::time::Instant::now();
        let status = login_status(
            &fake_claude(dir.path(), "exec >&-; sleep 30"),
            &env,
            Duration::from_secs(3),
        );
        assert!(matches!(status, LoginStatus::Unknown(_)), "{status:?}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "bounded by the timeout, not by the child: {:?}",
            started.elapsed()
        );
    }

    /// **An unknown shape is skipped; only a line that is not JSON is an
    /// error** — Claude Code adds event types, and a turn must not die of one.
    #[test]
    fn unknown_lines_are_skipped_and_garbage_is_said() {
        assert_eq!(
            read_line(r#"{"type":"something_new","x":1}"#),
            Ok(Vec::new())
        );
        assert_eq!(read_line("   "), Ok(Vec::new()));
        assert!(read_line("not json").is_err());
        assert!(read_line("[1,2]").is_err());
    }
}
