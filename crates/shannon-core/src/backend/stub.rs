//! Canned-response stub backend.
//!
//! Streams a fixed reply char-by-char (in chunks) over the standard
//! `Backend` channel. Exists to (1) prove the streaming + scroll
//! invariant without an API credential, (2) be the no-config fallback
//! when no API key is configured so `cargo run` always shows something
//! useful, and (3) keep the UI testable in isolation from network
//! state.
//!
//! Two knobs (`latency`, `chunk_size`) control visible cadence;
//! effective char rate is `chunk_size / latency`. Bigger chunks are
//! the cheaper path — same wake-ups, more chars per tick, closer to
//! what a real SSE backend looks like. Default `(60ms, 4 chars)` is a
//! deliberate choice the user landed on after testing several values.
//!
//! **It asks for the tool calls you write** (F2d, `file-tools.md`): a
//! fenced block whose info string is `tool <name>` is a call to that tool —
//! `bash`'s body the command as it stands, any other tool's its input as a
//! JSON object — so the gate, the pane, the run and the record can be
//! driven with nothing spent (Steven's tutor draft, 11-03: "Do we have a way
//! to force the tool call with the stub? … this seems like it is also a
//! useful debugging tool"). Answered, it says what each call returned as the
//! model was handed it: redacted, a refusal's reason, a link's landing.

use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tokio::time::sleep;

use super::{Backend, SendRequest};
use crate::{BASH_TOOL, Block, Message, Role, StopReason, StreamMsg, Usage};

pub struct CannedStub {
    latency: Duration,
    chunk_size: usize,
}

impl CannedStub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Customize cadence. Bigger chunk size or smaller latency makes
    /// the stub feel snappier; both are useful for stress-testing the
    /// TUI's stream-during-scroll behavior.
    pub fn with_cadence(latency: Duration, chunk_size: usize) -> Self {
        Self {
            latency,
            chunk_size,
        }
    }
}

impl Default for CannedStub {
    fn default() -> Self {
        Self {
            latency: Duration::from_millis(60),
            chunk_size: 4,
        }
    }
}

impl Backend for CannedStub {
    fn send_message(&self, req: SendRequest, tx: UnboundedSender<StreamMsg>) -> JoinHandle<()> {
        let latency = self.latency;
        let chunk_size = self.chunk_size.max(1);
        // The stub ignores model / max_tokens overrides; it reads only the
        // conversation.
        let reply = reply_to(&req.messages, latency, chunk_size);
        tokio::spawn(async move {
            let chars: Vec<char> = reply.text.chars().collect();
            // Synthetic usage so the token display is exercisable offline
            // (~4 chars/token, the usual rough rule). Sent up front like the
            // real `message_start`, before the chunks.
            let usage = Usage {
                input_tokens: (reply.read / 4) as u32,
                output_tokens: (chars.len() / 4) as u32,
                // The stub does not cache: it has no provider and no prefix to
                // share, and reporting a hit it never had would put a number on
                // screen that no request produced.
                ..Usage::default()
            };
            let _ = tx.send(StreamMsg::Usage(usage));
            for group in chars.chunks(chunk_size) {
                let s: String = group.iter().collect();
                if tx.send(StreamMsg::Chunk(s)).is_err() {
                    return;
                }
                sleep(latency).await;
            }
            // A call after the text, as the API streams a `tool_use` block
            // after the text before it; the turn then stops on it.
            let stop = match reply.calls.is_empty() {
                true => StopReason::EndTurn,
                false => StopReason::ToolUse,
            };
            for call in reply.calls {
                if tx.send(StreamMsg::ToolUse(call)).is_err() {
                    return;
                }
            }
            // Mirror the real wire order: `message_delta` (stop reason) then
            // `message_stop` — so the offline stub exercises the same
            // provenance path as the API, and tests can see stop reasons.
            let _ = tx.send(StreamMsg::Stopped(stop));
            let _ = tx.send(StreamMsg::Complete);
        })
    }

    fn model(&self) -> &str {
        // A recognizable sentinel: turns produced offline show "canned-stub" in
        // the viewer, so a resumed session makes plain which turns came from the
        // stub vs. a real model (the exact "stub junk mixed in" confusion).
        //
        // **The constant, not the literal.** `App::only_stub` claims it reads
        // this "by construction" — which was two independently typed strings in
        // two crates until this line named the constant (Reviewer C). Now the
        // claim is true rather than merely lucky.
        crate::backend::models::STUB
    }
}

/// **What the stub says to a request** — its text, the calls it asks for
/// after it, and how many characters it read (its input's token count).
struct Reply {
    text: String,
    calls: Vec<Block>,
    read: usize,
}

/// **The stub's answer to `messages`**, from the last of your side's: a
/// round's results said back ([`answered`]); a prompt writing out calls,
/// asked for ([`calls_in`]) — or, a call it cannot read, said and none
/// asked for; any other prompt echoed ([`canned_for`]).
fn reply_to(messages: &[Message], latency: Duration, chunk_size: usize) -> Reply {
    let last = messages.iter().rev().find(|m| m.role == Role::User);
    let results: Vec<(&str, &str, bool)> = (last.into_iter())
        .flat_map(|m| &m.blocks)
        .filter_map(|b| match b {
            Block::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some((tool_use_id.as_str(), content.as_str(), *is_error)),
            _ => None,
        })
        .collect();
    if !results.is_empty() {
        let read = results.iter().map(|(_, c, _)| c.chars().count()).sum();
        let text = answered(messages, &results);
        let calls = Vec::new();
        return Reply { text, calls, read };
    }
    let prompt = last.map_or("", |m| m.content.as_str());
    let read = prompt.chars().count();
    let (text, calls) = match calls_in(prompt) {
        Ok(calls) if calls.is_empty() => (canned_for(prompt, latency, chunk_size), calls),
        Ok(calls) => {
            let names: Vec<String> = (calls.iter())
                .filter_map(|c| c.tool_name().map(|name| format!("`{name}`")))
                .collect();
            (
                format!("Calling what you wrote: {}.\n", names.join(", ")),
                calls,
            )
        }
        Err(why) => (format!("I made no call: {why}.\n"), Vec::new()),
    };
    Reply { text, calls, read }
}

/// **The calls a prompt writes out** — each fenced block (three or more
/// backticks or tildes) whose info string is `tool <name>`, in order:
/// `bash`'s body is its command as it stands, a heredoc and all; any other
/// tool's is its input, a JSON object. One that is not refuses them all, a
/// call it cannot read being one it must not make (as
/// [`Block::bash_command`]). A plain ```` ```bash ```` block is code, not a
/// call. Each has an id of its own ([`call_id`]).
///
/// **Every block is read whole, a call or not**, so a call quoted inside
/// another block — a longer fence showing how one is written, a tool's
/// result pasted back — is the outer block's text, never a call (review
/// stop 16, E 11, measured: a quoted ```` ```tool bash ```` fired).
fn calls_in(prompt: &str) -> Result<Vec<Block>, String> {
    let mut calls = Vec::new();
    let mut lines = prompt.lines();
    while let Some(line) = lines.next() {
        let Some((fence, info)) = opens_a_fence(line) else {
            continue;
        };
        // The fence ends at a line of its mark at least as long, or at the
        // prompt's end, as CommonMark's does.
        let body: Vec<&str> = (lines.by_ref())
            .take_while(|line| !closes(line, fence))
            .collect();
        let Some(name) = tool_named(info) else {
            continue;
        };
        let body = body.join("\n");
        let input = match name {
            BASH_TOOL => serde_json::json!({ "command": body }),
            _ => match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(input @ serde_json::Value::Object(_)) => input,
                Ok(_) => return Err(format!("the `tool {name}` block is not a JSON object")),
                Err(why) => return Err(format!("the `tool {name}` block is not JSON ({why})")),
            },
        };
        calls.push((name.to_string(), input));
    }
    // Ids only once every block is read: one unreadable makes no call, and
    // spends none.
    Ok((calls.into_iter())
        .map(|(name, input)| Block::ToolUse {
            id: call_id(),
            name,
            input,
        })
        .collect())
}

/// **An id no other call has had** — in this run, or a run before it whose
/// session was reopened. The app refuses a call whose id it has issued
/// (`App::claim_tool_id`), so ids by turn (`toolu_stub_1_0`) made a second
/// stub session's first call a refused duplicate of the first one's: this
/// run's start, and a count.
fn call_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static RUN: std::sync::OnceLock<u128> = std::sync::OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let run = RUN.get_or_init(|| {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        now.map_or(0, |since| since.as_nanos())
    });
    format!(
        "toolu_stub_{run:x}_{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A line opening a fenced block — its fence, three or more backticks or
/// tildes, and the info string after it.
fn opens_a_fence(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    let mark = line.chars().next().filter(|&c| c == '`' || c == '~')?;
    let long = line.chars().take_while(|&c| c == mark).count();
    // The mark is ASCII: `long` chars are `long` bytes.
    (long >= 3).then(|| line.split_at(long))
}

/// The tool a block's info string calls, as `tool bash` does, with nothing
/// after the name.
fn tool_named(info: &str) -> Option<&str> {
    let mut words = info.split_whitespace();
    let name = words.next().filter(|&w| w == "tool").and(words.next())?;
    words.next().is_none().then_some(name)
}

/// A line opening a call's block — its fence and the tool it names, as
/// in ```` ```tool bash ````.
#[cfg(test)]
fn opens_a_call(line: &str) -> Option<(&str, &str)> {
    let (fence, info) = opens_a_fence(line)?;
    Some((fence, tool_named(info)?))
}

/// Does `line` close the block `fence` opened — its mark alone, at least as
/// many?
fn closes(line: &str, fence: &str) -> bool {
    let line = line.trim();
    let mark = fence.chars().next();
    line.len() >= fence.len() && line.chars().all(|c| Some(c) == mark)
}

/// **What the stub says once its calls are answered** — each result as the
/// model was handed it, by the tool it answers, so what reached the model
/// (redacted, a refusal's reason, a link's landing) is read in the reply.
fn answered(messages: &[Message], results: &[(&str, &str, bool)]) -> String {
    let named = |id: &str| {
        (messages.iter().flat_map(Message::tool_uses))
            .find(|&(at, _, _)| at == id)
            .map_or("The tool".to_string(), |(_, name, _)| format!("`{name}`"))
    };
    let said: Vec<String> = (results.iter())
        .map(|&(id, content, is_error)| {
            let how = match is_error {
                true => "answered, as an error",
                false => "answered",
            };
            match content.is_empty() {
                true => format!("{} {how} with nothing.\n", named(id)),
                false => {
                    let fence = fence_for(content);
                    format!("{} {how}:\n\n{fence}\n{content}\n{fence}\n", named(id))
                }
            }
        })
        .collect();
    said.join("\n")
}

/// A backtick fence longer than any run of backticks in `text`, so the
/// text is held whole (CommonMark's rule for a fenced block).
fn fence_for(text: &str) -> String {
    let longest = (text.split(|c| c != '`')).map(str::len).max().unwrap_or(0);
    "`".repeat(longest.max(2) + 1)
}

/// Compose the canned text. Echoes the full prompt verbatim so the
/// reply grows with the input — useful when stress-testing scroll. The prompt
/// is echoed on **its own lines** (not glued after a prefix), so a code fence
/// at the very start of the input lands at the start of a line and the
/// renderer's markdown/syntax highlighting sees it — matching how a real
/// assistant reply is structured.
///
/// The echo is what the stub was SENT, so a secret in it is a handle — and the
/// display shows a held handle as `value (handle)`, both marked, which is the
/// "show both" a test of the boundary wants (Steven, 10-02). A separate "on
/// the wire" line the stub printed for a few commits said the same thing twice.
///
/// **What you would type is code** — the variables and the keys, in
/// backticks, so the transcript's markdown draws them as an inline code
/// span: bare, they drew as the prose around them (Steven, 11-08: "doesn't
/// highlight the environment variables or the commands";
/// `what_you_type_in_the_canned_text_is_code`).
///
/// **The way to a real model is `:setup`**, which the README and the
/// tutor teach and which works where an environment variable cannot: in
/// `shannon tutor`, already running, the reply told you to set
/// `ANTHROPIC_API_KEY` (review stop 17, the tutor's fact check).
fn canned_for(prompt: &str, latency: Duration, chunk_size: usize) -> String {
    let ms = latency.as_millis();
    format!(
        "Got it. You said:\n\n{prompt}\n\nThis is the canned stub backend — {ms} ms between chunks, {chunk_size} char(s) per chunk. `:setup` chooses a real model: an API key, or your own `claude`. Try `PgUp` / `PgDn` or `Ctrl-U` / `Ctrl-D` while it's still streaming; the viewport should stay where you put it.\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use crate::backend::contract::the_ending;
    use tokio::sync::mpsc;

    /// **What you would type in the canned text is code**: outside its
    /// backtick spans, no word shaped like an environment variable
    /// (`ANTHROPIC_API_KEY`) or a key (`PgUp`, `Ctrl-U`, `<C-u>`) — the
    /// transcript draws a code span as code and the rest as prose.
    #[test]
    fn what_you_type_in_the_canned_text_is_code() {
        let text = canned_for("", Duration::from_millis(60), 4);
        let prose: String = (text.split('`').step_by(2)).collect::<Vec<_>>().join(" ");
        let typed = |word: &str| {
            let variable = word.len() > 2
                && word.contains('_')
                && word
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            let key = ["PgUp", "PgDn", "Ctrl-", "CTRL-", "<C-"]
                .iter()
                .any(|k| word.starts_with(k));
            variable || key
        };
        let bare: Vec<&str> = (prose.split(|c: char| c.is_whitespace() || ",.;/".contains(c)))
            .filter(|word| typed(word))
            .collect();
        assert_eq!(bare, Vec::<&str>::new(), "{text}");
        assert!(
            text.contains("`PgUp`") && text.contains("`:setup`"),
            "premise: it names a key, and the way to a real model"
        );
    }

    #[test]
    fn canned_echoes_full_prompt() {
        let long = "x".repeat(200);
        let out = canned_for(&long, Duration::from_millis(60), 4);
        assert!(out.contains(&long), "full prompt must appear verbatim");
        assert!(!out.contains('…'), "no ellipsis should be inserted");
    }

    /// **The stub's stream has the API's shape** — usage first, then text
    /// alone, then the stop reason and the one ending — so the offline path
    /// exercises the provenance the real one does. Its "mirror the real wire
    /// order" was unasserted: never sending `Stopped` passed both crates
    /// (review stop 12, T9 4).
    #[tokio::test]
    async fn spawn_emits_chunks_then_complete() {
        let stub = CannedStub::with_cadence(Duration::from_millis(1), 8);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _h = stub.send_message(SendRequest::new(vec![Message::user("hi")]), tx);
        let mut msgs = Vec::new();
        let wait = Duration::from_secs(5);
        while let Some(msg) =
            (tokio::time::timeout(wait, rx.recv()).await).expect("the stub stalled")
        {
            msgs.push(msg);
        }
        assert_eq!(the_ending(&msgs), &StreamMsg::Complete);
        assert!(matches!(msgs[0], StreamMsg::Usage(_)), "{msgs:?}");
        let stop = msgs.len() - 2;
        assert_eq!(msgs[stop], StreamMsg::Stopped(StopReason::EndTurn));
        let assembled: String = (msgs[1..stop].iter())
            .map(|msg| match msg {
                StreamMsg::Chunk(text) => text.as_str(),
                other => panic!("only text between: {other:?}"),
            })
            .collect();
        // The prompt is echoed on its own line (`You said:\n\nhi`), so a code
        // fence at the start of the input lands at a line start for the renderer.
        assert!(assembled.contains("You said:\n\nhi\n"));
        assert!(assembled.ends_with('\n'));
    }

    /// The stub's whole stream for `messages`, to its ending.
    async fn streamed(messages: Vec<Message>) -> Vec<StreamMsg> {
        let stub = CannedStub::with_cadence(Duration::from_millis(1), 64);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _h = stub.send_message(SendRequest::new(messages), tx);
        let mut msgs = Vec::new();
        let wait = Duration::from_secs(5);
        while let Some(msg) =
            (tokio::time::timeout(wait, rx.recv()).await).expect("the stub stalled")
        {
            msgs.push(msg);
        }
        msgs
    }

    /// The text a stream says.
    fn text_of(msgs: &[StreamMsg]) -> String {
        (msgs.iter())
            .filter_map(|msg| match msg {
                StreamMsg::Chunk(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// **A `tool` block is a call** (F2d) — `bash`'s body its command, a
    /// heredoc's lines and all; another tool's a JSON object, under a
    /// tilde fence too — asked for after the text, and the turn stops on
    /// them; a plain `bash` block is code, no call.
    #[tokio::test]
    async fn a_tool_block_asks_for_its_call() {
        let prompt = "Run these:\n```tool bash\ncat <<EOF\nhi\nEOF\n```\n\
                      ~~~~tool Read\n{\"file_path\": \"a.txt\"}\n~~~~\n```bash\nls\n```\n";
        let msgs = streamed(vec![Message::user(prompt)]).await;
        let calls: Vec<&Block> = (msgs.iter())
            .filter_map(|msg| match msg {
                StreamMsg::ToolUse(call) => Some(call),
                _ => None,
            })
            .collect();
        assert_eq!(calls.len(), 2, "{msgs:?}");
        assert_eq!(calls[0].bash_command(), Some("cat <<EOF\nhi\nEOF"));
        let Block::ToolUse { id, name, input } = calls[1] else {
            panic!("a call: {:?}", calls[1]);
        };
        assert_eq!(name, "Read");
        assert_eq!(input, &serde_json::json!({"file_path": "a.txt"}));
        // **Every id its own**, in a request and across them: the app refuses
        // a call whose id it has issued, so a second session's first call
        // was refused as the first one's (`toolu_stub_1_0` each).
        let again = streamed(vec![Message::user(prompt)]).await;
        let ids: std::collections::HashSet<&str> = (msgs.iter().chain(&again))
            .filter_map(|msg| match msg {
                StreamMsg::ToolUse(Block::ToolUse { id, .. }) => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 4, "{ids:?}");
        assert!(id.starts_with("toolu_stub_"), "{id}");
        assert_eq!(the_ending(&msgs), &StreamMsg::Complete);
        let stop = msgs.len() - 2;
        assert_eq!(msgs[stop], StreamMsg::Stopped(StopReason::ToolUse));
        assert!(
            matches!(msgs[stop - 1], StreamMsg::ToolUse(_)),
            "after the text"
        );
        let text = text_of(&msgs);
        assert!(
            text.contains("`bash`, `Read`") && !text.contains("You said"),
            "{text}"
        );
    }

    /// **A call it cannot read makes none** — not JSON, or not an object:
    /// said, and the turn ends with no call, the readable one beside it
    /// included.
    #[tokio::test]
    async fn a_tool_block_it_cannot_read_makes_no_call() {
        for (body, why) in [("not json", "is not JSON"), ("[1, 2]", "not a JSON object")] {
            let prompt = format!("```tool bash\nls\n```\n```tool Read\n{body}\n```\n");
            let msgs = streamed(vec![Message::user(prompt)]).await;
            let called = msgs.iter().any(|m| matches!(m, StreamMsg::ToolUse(_)));
            assert!(!called, "{msgs:?}");
            let text = text_of(&msgs);
            assert!(
                text.contains("I made no call") && text.contains(why),
                "{text}"
            );
            let stop = &msgs[msgs.len() - 2];
            assert_eq!(stop, &StreamMsg::Stopped(StopReason::EndTurn));
        }
    }

    /// **Answered, it says what the model was handed** — by the tool asked,
    /// an error said as one, a result holding a fence held whole by a longer
    /// one; and an empty one said empty.
    #[tokio::test]
    async fn an_answered_call_is_said_as_the_model_was_handed_it() {
        let mut asked = Message::assistant("Calling what you wrote: `bash`.\n");
        asked.blocks.push(Block::bash_call("toolu_stub_1_0", "ls"));
        let answer = |content: &str, is_error| {
            vec![
                Message::user("```tool bash\nls\n```"),
                asked.clone(),
                Message::tool_result("toolu_stub_1_0", content, is_error),
            ]
        };
        let text = text_of(&streamed(answer("a\n```\nb", true)).await);
        let said = "`bash` answered, as an error:\n\n````\na\n```\nb\n````\n";
        assert!(text.contains(said) && !text.contains("You said"), "{text}");
        let text = text_of(&streamed(answer("", false)).await);
        assert_eq!(text, "`bash` answered with nothing.\n");
    }

    /// **What opens a call's block, and what closes it** — three marks or
    /// more, `tool`, one name and nothing after; closed by a line of the
    /// mark alone, at least as long.
    #[test]
    fn what_opens_and_closes_a_call() {
        assert_eq!(opens_a_call("```tool bash"), Some(("```", "bash")));
        assert_eq!(opens_a_call("  ````tool  Read "), Some(("````", "Read")));
        for not in [
            "``tool bash",
            "```tool",
            "```tool bash now",
            "```bash",
            "``` tools x",
        ] {
            assert_eq!(opens_a_call(not), None, "{not:?}");
        }
        assert!(closes("```", "```") && closes(" ````", "```"));
        assert!(!closes("``", "```") && !closes("~~~", "```") && !closes("``` x", "```"));
    }

    /// **A call quoted inside another block is the block's text, not a
    /// call** (review stop 16, E 11, measured: a quoted ```` ```tool bash ````
    /// ran `touch pwned`) — inside a longer fence, a tilde fence, a code
    /// block; the call written after them still is one.
    #[test]
    fn a_call_quoted_inside_another_block_is_not_one() {
        for quoted in [
            "````markdown\n```tool bash\ntouch pwned\n```\n````",
            "~~~\n```tool bash\ntouch pwned\n```\n~~~",
            "```text\nhow to: ```tool bash\n```",
        ] {
            assert_eq!(calls_in(quoted).unwrap(), Vec::new(), "{quoted:?}");
            let then = format!("{quoted}\n```tool bash\nls\n```\n");
            let made = calls_in(&then).unwrap();
            assert_eq!(made.len(), 1, "{then:?}");
            assert_eq!(made[0].bash_command(), Some("ls"), "{then:?}");
        }
    }

    #[tokio::test]
    async fn empty_history_does_not_panic() {
        let stub = CannedStub::with_cadence(Duration::from_millis(1), 8);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _h = stub.send_message(SendRequest::new(Vec::new()), tx);
        // Drain to completion; just verify it doesn't crash.
        loop {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Some(StreamMsg::Complete)) | Ok(None) => break,
                Ok(Some(_)) => continue,
                Err(_) => panic!("stub stalled with empty history"),
            }
        }
    }
}

/// A stub that **records what it was asked for** and streams nothing — not
/// even an ending: a test on it is the stream, feeding the app each message,
/// the ending included (`Backend::send_message`'s rule).
///
/// The `CannedStub` deliberately ignores model / max_tokens, which makes it
/// blind to exactly the class of bug where a request is built without the
/// session's settings — `send_foreground` carried no model at all until
/// 2026-08-07 and nothing could see it, because every session ran the backend
/// default and the two only diverged once `:model` existed. A double that
/// records the request is the only way to assert the wiring rather than the
/// outcome.
#[derive(Clone, Default)]
#[cfg(any(test, feature = "test-util"))]
pub struct RecordingStub {
    /// Every request received, in order — the whole request, so a knob
    /// beside the model (the ceiling, the effort) is as visible as it is.
    pub requests: std::sync::Arc<std::sync::Mutex<Vec<SendRequest>>>,
}

#[cfg(any(test, feature = "test-util"))]
impl RecordingStub {
    pub fn new() -> Self {
        Self::default()
    }

    /// The model on the most recent request, if there has been one.
    pub fn last_model(&self) -> Option<String> {
        self.requests
            .lock()
            .unwrap()
            .last()
            .and_then(|r| r.model.clone())
    }
}

/// A backend whose request task **never finishes**, and which reports when that
/// task is cancelled.
///
/// Exists to pin one thing the other stubs structurally cannot: that abandoning
/// a generation actually **stops the request**. Every other stub's task
/// completes on its own, so aborting it and not aborting it are
/// indistinguishable — which is why "an abandoned turn kept streaming and
/// billing" shipped twice, once as a forwarder handle that had swallowed the
/// request handle, and once as `pending_continuation` dropping its handle
/// (dropping a `JoinHandle` DETACHES rather than cancels).
///
/// The flag is set from a `Drop` guard *inside* the task, so it flips exactly
/// when the future is dropped — no sleeping, no timing margin to guess at
/// (`[[feedback_mutation_check_guards]]`: a margin that is assumed rather
/// than measured is a slow way to assert nothing).
#[cfg(any(test, feature = "test-util"))]
#[derive(Default)]
pub struct HangingBackend {
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(any(test, feature = "test-util"))]
impl HangingBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Has the request task been cancelled (or the runtime torn down)?
    ///
    /// The task is `pending` forever, so this can only become true by
    /// cancellation — there is no path where it completes normally.
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Sets the flag when dropped — including when the task holding it is aborted.
#[cfg(any(test, feature = "test-util"))]
struct CancelFlag(std::sync::Arc<std::sync::atomic::AtomicBool>);

#[cfg(any(test, feature = "test-util"))]
impl Drop for CancelFlag {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(any(test, feature = "test-util"))]
impl Backend for HangingBackend {
    fn send_message(&self, _req: SendRequest, tx: UnboundedSender<StreamMsg>) -> JoinHandle<()> {
        let flag = CancelFlag(self.cancelled.clone());
        tokio::spawn(async move {
            // `tx` is held so the forwarder bridging this stream stays alive
            // too: a forwarder that ended on its own would hide whether it was
            // aborted, which is the other half of the pair being tested.
            let _tx = tx;
            let _flag = flag;
            std::future::pending::<()>().await;
        })
    }

    fn model(&self) -> &str {
        "hanging-stub"
    }
}

/// **A provider that runs its own tools** (`claude -p`), standing in: it
/// records each request it is sent and each answer the gate gives it, and
/// runs nothing — so a test app can never start the real `claude`, and a test
/// can assert what the gate answered rather than what it showed. Like
/// [`RecordingStub`] it sends nothing, not even an ending: the test is the
/// stream.
#[cfg(any(test, feature = "test-util"))]
#[derive(Clone, Default)]
pub struct RecordingProvider {
    /// Every request received, in order.
    pub requests: std::sync::Arc<std::sync::Mutex<Vec<SendRequest>>>,
    /// Every answer to a tool question, in order: `(tool_use_id, reply)`.
    pub answers: std::sync::Arc<std::sync::Mutex<Vec<(String, crate::hook::Reply)>>>,
    /// The question each answer went to, in the same order — its
    /// [`AskId`](crate::AskId), serial and all: what an answer is routed by.
    pub asked: std::sync::Arc<std::sync::Mutex<Vec<crate::AskId>>>,
    /// Questions nobody waits for any more — an answer to one is recorded
    /// but reports undelivered, as the real backend's does once the hook is
    /// gone.
    pub gone: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

#[cfg(any(test, feature = "test-util"))]
impl RecordingProvider {
    pub fn new() -> Self {
        Self::default()
    }

    /// The answers given so far.
    pub fn answered(&self) -> Vec<(String, crate::hook::Reply)> {
        self.answers.lock().unwrap().clone()
    }
}

#[cfg(any(test, feature = "test-util"))]
impl Backend for RecordingProvider {
    fn send_message(&self, req: SendRequest, _tx: UnboundedSender<StreamMsg>) -> JoinHandle<()> {
        self.requests.lock().unwrap().push(req);
        tokio::spawn(async {})
    }

    fn model(&self) -> &str {
        "claude-code/recording"
    }

    fn answer_tool(&self, ask: &crate::AskId, reply: crate::hook::Reply) -> bool {
        let id = ask.tool_use_id();
        self.answers.lock().unwrap().push((id.to_string(), reply));
        let mut asked = self.asked.lock().unwrap();
        // A question answered once waits no more, as the real gate's does: a
        // second answer to one serial is not delivered — so a test can tell
        // an answer sent to the wrong question (review stop 7, Reviewer E 8).
        let again = asked.contains(ask);
        asked.push(ask.clone());
        !again && !self.gone.lock().unwrap().iter().any(|gone| gone == id)
    }
}

#[cfg(any(test, feature = "test-util"))]
impl Backend for RecordingStub {
    fn send_message(&self, req: SendRequest, _tx: UnboundedSender<StreamMsg>) -> JoinHandle<()> {
        self.requests.lock().unwrap().push(req);
        tokio::spawn(async {})
    }

    fn model(&self) -> &str {
        "recording-stub-default"
    }
}
