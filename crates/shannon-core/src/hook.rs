//! **`shannon hook` — Shannon's gate in front of Claude Code's tools.**
//!
//! Claude Code runs its own tool loop in a `claude -p` turn, and spawns a
//! PreToolUse hook for every tool call it is about to make
//! (`docs/DesignDocs/claude-code-backend.md`). This is that hook: it reads the
//! call on stdin, asks the running Shannon over a unix socket, and answers
//! Claude Code — allow, or a refusal.
//!
//! **It fails closed, and that is the whole design.** Claude Code's docs:
//! "A timed-out `command` … hook doesn't block the tool call", and "Any other
//! exit code [than 2] doesn't block on its own … the action proceeds". So a
//! hook that crashes, hangs, or exits 1 is a hook that ALLOWED. Here every
//! path but an explicit allow from Shannon exits 2 — "Exit 2 … blocks the tool
//! call", "even a JSON `permissionDecision` of `"allow"` can't override it" —
//! with the reason on stderr, which Claude Code hands to the model:
//! - no socket named, or none there to answer;
//! - an IO error, a reply that does not parse, a connection that closes;
//! - its own deadline, set below the hook's configured `timeout`, so it
//!   refuses before Claude Code could time it out and proceed;
//! - a panic ([`fail_closed`]).
//!
//! The wire is one JSON line each way: the call Claude Code gave us, forwarded
//! whole ([`Request`]), and Shannon's answer ([`Reply`]).

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The environment variable naming Shannon's socket — set by Shannon when it
/// spawns `claude`, inherited by the hook ("A hook process inherits the
/// parent environment").
pub const SOCKET_ENV: &str = "SHANNON_HOOK_SOCKET";

/// The environment variable carrying the hook's own deadline, in seconds —
/// below the `timeout` its settings give it, so it always answers first.
pub const DEADLINE_ENV: &str = "SHANNON_HOOK_DEADLINE";

/// The deadline when none is given: under Claude Code's default `timeout` of
/// 600 seconds, by a margin.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(570);

/// **What the hook asks Shannon**: the call exactly as Claude Code gave it —
/// `tool_name`, `tool_input`, `tool_use_id`, `cwd`, `session_id` — forwarded
/// whole, so Shannon decides on what Claude Code will run, not on a summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub call: serde_json::Value,
}

/// **Shannon's answer.** Anything else — no answer at all — is a refusal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "lowercase")]
pub enum Reply {
    /// Run it — as given, or with its input replaced (`e`, edit the command:
    /// Claude Code's `updatedInput` "replaces a tool's arguments before it
    /// runs").
    Allow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_input: Option<serde_json::Value>,
    },
    /// Do not; the reason goes to the model.
    Deny { reason: String },
}

/// What the hook process does: its exit code and what it writes. A value, so
/// every path is testable without spawning anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub exit: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Outcome {
    /// A refusal: exit 2, the reason on stderr — the one exit Claude Code
    /// treats as blocking.
    fn refuse(reason: impl Into<String>) -> Self {
        Self {
            exit: 2,
            stdout: String::new(),
            stderr: format!("Shannon refused this tool call: {}", reason.into()),
        }
    }

    /// An allow: the one JSON this hook ever prints, exit 0.
    fn allow(updated_input: Option<serde_json::Value>) -> Self {
        let mut decision = serde_json::json!({
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "allowed by Shannon",
        });
        if let Some(input) = updated_input {
            decision["updatedInput"] = input;
        }
        Self {
            exit: 0,
            stdout: serde_json::json!({ "hookSpecificOutput": decision }).to_string(),
            stderr: String::new(),
        }
    }
}

/// **The hook, as a function**: the call on `stdin`, the socket and deadline
/// from the environment as given, the answer as an [`Outcome`]. Every path
/// but an explicit [`Reply::Allow`] refuses.
#[cfg(unix)]
pub fn decide(stdin: impl Read, socket: Option<&Path>, deadline: Duration) -> Outcome {
    let call = match read_call(stdin) {
        Ok(call) => call,
        Err(refused) => return refused,
    };
    let Some(socket) = socket else {
        return Outcome::refuse(format!(
            "no Shannon to ask (`{SOCKET_ENV}` is not set) — this hook only answers for a \
             `claude -p` turn Shannon started"
        ));
    };
    match std::os::unix::net::UnixStream::connect(socket) {
        Ok(stream) => answered(exchange(stream, &Request { call }, deadline)),
        Err(e) => Outcome::refuse(format!(
            "Shannon could not be reached at {} ({e})",
            socket.display()
        )),
    }
}

#[cfg(not(unix))]
pub fn decide(_: impl Read, _: Option<&Path>, _: Duration) -> Outcome {
    Outcome::refuse("the hook needs a unix socket, which this platform lacks")
}

/// The call Claude Code gave, as a JSON object — or the refusal it earns.
fn read_call(stdin: impl Read) -> Result<serde_json::Value, Outcome> {
    let mut text = String::new();
    if let Err(e) = BufReader::new(stdin).read_to_string(&mut text) {
        return Err(Outcome::refuse(format!("the call could not be read ({e})")));
    }
    match serde_json::from_str(&text) {
        Ok(call @ serde_json::Value::Object(_)) => Ok(call),
        Ok(_) | Err(_) => Err(Outcome::refuse("the call was not a JSON object")),
    }
}

/// Shannon's answer, or why there was none, as what the hook does.
fn answered(reply: Result<Reply, String>) -> Outcome {
    match reply {
        Ok(Reply::Allow { updated_input }) => Outcome::allow(updated_input),
        Ok(Reply::Deny { reason }) => Outcome::refuse(reason),
        Err(why) => Outcome::refuse(why),
    }
}

/// One question over a connected stream, bounded by `deadline` for the whole
/// of it.
#[cfg(unix)]
fn exchange(
    mut stream: std::os::unix::net::UnixStream,
    request: &Request,
    deadline: Duration,
) -> Result<Reply, String> {
    let started = std::time::Instant::now();
    let left = |what: &str| {
        deadline
            .checked_sub(started.elapsed())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| format!("no answer within {}s ({what})", deadline.as_secs()))
    };
    stream
        .set_write_timeout(Some(left("sending")?))
        .map_err(|e| format!("socket: {e}"))?;
    let mut line = serde_json::to_string(request).map_err(|e| format!("encoding: {e}"))?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .map_err(|e| format!("sending the call: {e}"))?;
    await_answer(stream, left("waiting")?, deadline)
}

/// **Read Shannon's answer, waiting at most `wait`.** A Shannon that answers
/// and hangs up before the timeout is set leaves a socket macOS will not set
/// one on — `EINVAL`, though the answer is there, unread (probed 10-16) — and
/// the hook refused the call, an allow included: "socket: Invalid argument",
/// found as a test failing under load. A read there cannot block — the data,
/// then end of file — so it needs no timeout. Any other failure still refuses:
/// std's own `InvalidInput` for a zero `wait` carries no OS error, and a read
/// with no timeout on a live socket could wait forever.
fn await_answer(
    stream: std::os::unix::net::UnixStream,
    wait: Duration,
    deadline: Duration,
) -> Result<Reply, String> {
    match stream.set_read_timeout(Some(wait)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput && e.raw_os_error().is_some() => {}
        Err(e) => return Err(format!("socket: {e}")),
    }
    let mut answer = String::new();
    match BufReader::new(stream).read_line(&mut answer) {
        Ok(0) => Err("Shannon closed the question without answering".into()),
        Ok(_) => {
            serde_json::from_str(&answer).map_err(|e| format!("an answer that did not parse ({e})"))
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            Err(format!("no answer within {}s", deadline.as_secs()))
        }
        Err(e) => Err(format!("waiting for the answer: {e}")),
    }
}

/// The deadline the environment gives, or [`DEFAULT_DEADLINE`]. A value that
/// does not parse is not trusted to mean "forever": the default stands.
pub fn deadline_from(value: Option<&str>) -> Duration {
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&secs| secs > 0)
        .map_or(DEFAULT_DEADLINE, Duration::from_secs)
}

/// **Run `body`, and turn a panic into a refusal.** A Rust panic exits 101,
/// which Claude Code treats as a non-blocking error — the tool would RUN. So
/// the hook's whole body runs under this, and a panic is exit 2.
pub fn fail_closed(body: impl FnOnce() -> Outcome + std::panic::UnwindSafe) -> Outcome {
    std::panic::catch_unwind(body)
        .unwrap_or_else(|_| Outcome::refuse("the hook failed (a panic), so nothing was allowed"))
}

/// **`shannon hook`, end to end** — the call read and decided under
/// [`fail_closed`], the outcome written by [`emit`], the exit code returned.
/// What `main` runs, here rather than in the binary because the binary is
/// tested by nothing: both guards survived deletion there (review 21,
/// Reviewer C).
///
/// What this cannot catch, an abort or a binary that will not start, the
/// hook's shell command does (`|| exit 2`, `backend::claude_code`).
pub fn run(
    stdin: impl Read,
    socket: Option<&Path>,
    deadline: Duration,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> i32 {
    let outcome = fail_closed(std::panic::AssertUnwindSafe(|| {
        decide(stdin, socket, deadline)
    }));
    emit(&outcome, stdout, stderr)
}

/// **Write `outcome`, and refuse an allow that does not reach Claude Code.**
/// Its stderr always; its stdout only for an allow, and an allow that cannot
/// be written exits 2 — never the 0 that would leave Claude Code with no
/// decision to read and its own rules to fall back on (`print!` on a closed
/// pipe panics, and a panic exits 101, which lets the tool run).
pub fn emit(outcome: &Outcome, stdout: &mut impl Write, stderr: &mut impl Write) -> i32 {
    let _ = stderr.write_all(outcome.stderr.as_bytes());
    if outcome.exit == 0
        && stdout
            .write_all(outcome.stdout.as_bytes())
            .and_then(|()| stdout.flush())
            .is_err()
    {
        return 2;
    }
    outcome.exit
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    const CALL: &str =
        r#"{"tool_name":"Bash","tool_input":{"command":"ls"},"tool_use_id":"toolu_1"}"#;

    /// The hook's side of one question to a Shannon that answers with `reply`
    /// (a raw line, so a test can send garbage) after `delay`, or closes
    /// without one. Over a socket PAIR — the sandbox this suite runs in may not
    /// bind a socket to a path, and the exchange is the same either way; only
    /// `connect` differs, and its failure is tested through `decide`.
    fn ask_shannon(reply: Option<&'static str>, delay: Duration, deadline: Duration) -> Outcome {
        let (hook, shannon) = UnixStream::pair().expect("pair");
        std::thread::spawn(move || {
            let mut asked = String::new();
            let mut reader = BufReader::new(shannon.try_clone().expect("clone"));
            let _ = reader.read_line(&mut asked);
            let got: Request = serde_json::from_str(&asked).expect("a request");
            assert_eq!(got.call["tool_name"], "Bash", "the call, forwarded whole");
            std::thread::sleep(delay);
            if let Some(reply) = reply {
                let mut shannon = shannon;
                let _ = shannon.write_all(format!("{reply}\n").as_bytes());
            }
        });
        match read_call(CALL.as_bytes()) {
            Ok(call) => answered(exchange(hook, &Request { call }, deadline)),
            Err(refused) => refused,
        }
    }

    fn json(out: &Outcome) -> serde_json::Value {
        serde_json::from_str(&out.stdout).expect("JSON stdout")
    }

    const QUICK: Duration = Duration::from_secs(5);

    /// **An answer given before the hook waits for it is read** (10-16): a
    /// Shannon that answers and hangs up at once left a socket macOS will not
    /// set a timeout on, and the hook refused the allow — "socket: Invalid
    /// argument", which `an_allow_is_the_one_json_it_prints` hit under load.
    /// Set up exactly here, not raced for. A zero wait still refuses: a read
    /// with no timeout on a live socket could wait forever.
    #[test]
    fn an_answer_given_before_the_wait_is_read_though_its_sender_hung_up() {
        let (hook, mut shannon) = UnixStream::pair().expect("pair");
        shannon.write_all(b"{\"answer\":\"allow\"}\n").unwrap();
        drop(shannon);
        assert!(
            matches!(
                await_answer(hook, QUICK, QUICK),
                Ok(Reply::Allow {
                    updated_input: None
                })
            ),
            "the allow it was sent"
        );

        // On a thread, bounded: the failure this guards is a read that never
        // returns, and a test that hung on it would say nothing.
        let (live, shannon) = UnixStream::pair().expect("pair");
        let (said, heard) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = said.send(await_answer(live, Duration::ZERO, QUICK).is_err());
        });
        let refused = heard
            .recv_timeout(Duration::from_secs(2))
            .expect("a zero wait answers at once, never reads unbounded");
        assert!(refused, "a zero wait refuses");
        drop(shannon);
    }

    #[test]
    fn an_allow_is_the_one_json_it_prints() {
        let out = ask_shannon(Some(r#"{"answer":"allow"}"#), Duration::ZERO, QUICK);
        assert_eq!(out.exit, 0, "{out:?}");
        let said = json(&out);
        assert_eq!(said["hookSpecificOutput"]["permissionDecision"], "allow");
        assert_eq!(said["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert!(said["hookSpecificOutput"].get("updatedInput").is_none());
    }

    #[test]
    fn an_edit_is_an_allow_with_the_input_replaced() {
        let out = ask_shannon(
            Some(r#"{"answer":"allow","updated_input":{"command":"ls -l"}}"#),
            Duration::ZERO,
            QUICK,
        );
        assert_eq!(out.exit, 0, "{out:?}");
        assert_eq!(
            json(&out)["hookSpecificOutput"]["updatedInput"]["command"],
            "ls -l"
        );
    }

    #[test]
    fn a_deny_exits_two_with_the_reason() {
        let out = ask_shannon(
            Some(r#"{"answer":"deny","reason":"rm is denied"}"#),
            Duration::ZERO,
            QUICK,
        );
        assert_eq!((out.exit, out.stdout.as_str()), (2, ""), "{out:?}");
        assert!(out.stderr.contains("rm is denied"), "{out:?}");
    }

    /// **Every way of not hearing "allow" is a refusal** — each of these, as
    /// any exit but 2, would let the tool run.
    #[test]
    fn every_failure_refuses() {
        let refused = |out: Outcome, what: &str| {
            assert_eq!(out.exit, 2, "{what}: {out:?}");
            assert!(out.stdout.is_empty(), "{what}: nothing on stdout");
            assert!(out.stderr.starts_with("Shannon refused"), "{what}: {out:?}");
        };
        refused(decide(CALL.as_bytes(), None, QUICK), "no socket named");
        let dir = tempfile::tempdir().expect("dir");
        let none = dir.path().join("none");
        refused(decide(CALL.as_bytes(), Some(&none), QUICK), "nobody there");
        refused(
            decide("not json".as_bytes(), Some(&none), QUICK),
            "bad call",
        );
        refused(
            decide("[1]".as_bytes(), Some(&none), QUICK),
            "not an object",
        );
        refused(
            ask_shannon(None, Duration::ZERO, QUICK),
            "closed without answering",
        );
        refused(
            ask_shannon(Some("garbage"), Duration::ZERO, QUICK),
            "an answer that does not parse",
        );
        refused(
            ask_shannon(Some(r#"{"answer":"maybe"}"#), Duration::ZERO, QUICK),
            "an answer that is not one",
        );
    }

    /// **Its own deadline refuses** — before Claude Code's `timeout`, which
    /// would let the tool run.
    #[test]
    fn the_deadline_refuses() {
        let started = std::time::Instant::now();
        let out = ask_shannon(
            Some(r#"{"answer":"allow"}"#),
            Duration::from_secs(3),
            Duration::from_millis(300),
        );
        assert_eq!(out.exit, 2, "{out:?}");
        assert!(out.stderr.contains("no answer within"), "{out:?}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "it waited past its deadline"
        );
    }

    #[test]
    fn a_panic_is_a_refusal() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let out = fail_closed(|| panic!("boom"));
        std::panic::set_hook(previous);
        assert_eq!(out.exit, 2, "{out:?}");
        assert!(out.stderr.contains("panic"), "{out:?}");
    }

    /// **The hook end to end refuses what it cannot finish**: a panic while
    /// deciding (here, in reading the call) exits 2 with nothing on stdout,
    /// through `run` — the path `main` takes, not `fail_closed` alone.
    #[test]
    fn a_panic_anywhere_in_the_hook_exits_2() {
        struct Boom;
        impl Read for Boom {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("the call could not be read");
            }
        }
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(Boom, None, Duration::from_secs(1), &mut out, &mut err);
        assert_eq!(code, 2);
        assert!(out.is_empty(), "nothing an allow could be read from");
        assert!(String::from_utf8_lossy(&err).contains("panic"));
    }

    /// **An allow that cannot be written is a refusal** — exit 2, not 0 with
    /// nothing for Claude Code to read. A refusal never touches stdout.
    #[test]
    fn an_allow_that_cannot_be_written_exits_2() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut err = Vec::new();
        assert_eq!(emit(&Outcome::allow(None), &mut Broken, &mut err), 2);
        let mut out = Vec::new();
        assert_eq!(emit(&Outcome::allow(None), &mut out, &mut err), 0);
        assert!(!out.is_empty(), "the allow was written");
        assert_eq!(emit(&Outcome::refuse("no"), &mut Broken, &mut err), 2);
    }

    #[test]
    fn a_deadline_is_read_and_never_taken_as_forever() {
        assert_eq!(deadline_from(Some("120")), Duration::from_secs(120));
        for bad in [None, Some(""), Some("0"), Some("-5"), Some("soon")] {
            assert_eq!(deadline_from(bad), DEFAULT_DEADLINE, "{bad:?}");
        }
    }
}
