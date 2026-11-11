//! **`shannon hook` as Claude Code runs it** — the real binary, a call on
//! stdin. Every path here must exit 2 with nothing on stdout: Claude Code lets
//! the tool run on any other exit, and reads stdout as the decision.
//!
//! The paths a test can reach without a Shannon listening; the answered paths
//! are `shannon_core::hook`'s unit tests. This holds `main`'s dispatch, which
//! no unit test reaches (`[[feedback_wiring_is_untested]]`).

use std::io::Write;
use std::process::{Command, Stdio};

const CALL: &str =
    r#"{"tool_name":"Bash","tool_input":{"command":"rm -rf /"},"tool_use_id":"toolu_1"}"#;

fn hook(stdin: &str, socket: Option<&str>) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_shannon"));
    cmd.arg("hook")
        .env_remove("SHANNON_HOOK_SOCKET")
        .env("SHANNON_HOOK_DEADLINE", "5")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(socket) = socket {
        cmd.env("SHANNON_HOOK_SOCKET", socket);
    }
    let mut child = cmd.spawn().expect("run shannon hook");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write the call");
    child.wait_with_output().expect("wait")
}

#[test]
fn with_no_shannon_to_ask_the_hook_refuses() {
    let dir = tempfile::tempdir().expect("dir");
    let nowhere = dir.path().join("hook.sock");
    for (stdin, socket, what) in [
        (CALL, None, "no socket named"),
        (CALL, nowhere.to_str(), "nobody at the socket"),
        ("not json", nowhere.to_str(), "a call that is not JSON"),
    ] {
        let out = hook(stdin, socket);
        assert_eq!(out.status.code(), Some(2), "{what}: {out:?}");
        assert!(
            out.stdout.is_empty(),
            "{what}: stdout is the decision: {out:?}"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("Shannon refused"),
            "{what}: {out:?}"
        );
    }
    // **Its own reason** — the call read before the socket is asked: every
    // case above exits 2, so the label "not JSON" was measured by nothing
    // (review stop 12, T7 17).
    let out = hook("not json", nowhere.to_str());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not a JSON object"),
        "{out:?}"
    );
}

/// **The deadline bounds the whole process** — a stdin held open never
/// reached `run`'s clock, and past Claude Code's own timeout the tool RUNS
/// (review 21, Reviewer B). Polled, so a regression fails instead of hanging.
///
/// **Refused AT the deadline, saying so**: exit 2 alone is every refusal's,
/// so a hook that refused at once for another reason passed (review stop 12,
/// T7 17). The watchdog's sentence, a second in.
#[test]
fn a_hook_still_waiting_at_its_deadline_refuses() {
    use std::io::Read;
    use std::time::{Duration, Instant};
    let mut child = Command::new(env!("CARGO_BIN_EXE_shannon"))
        .arg("hook")
        .env_remove("SHANNON_HOOK_SOCKET")
        .env("SHANNON_HOOK_DEADLINE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run shannon hook");
    let _held_open = child.stdin.take();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break status;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("still waiting 10 s into a 1 s deadline");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(2));
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "refused before its 1 s deadline: {:?}",
        started.elapsed()
    );
    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout")
        .read_to_end(&mut stdout)
        .expect("read");
    assert!(stdout.is_empty(), "stdout is the decision");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("stderr")
        .read_to_string(&mut stderr)
        .expect("read");
    assert!(
        stderr.contains("no decision within the hook's deadline"),
        "{stderr:?}"
    );
}
