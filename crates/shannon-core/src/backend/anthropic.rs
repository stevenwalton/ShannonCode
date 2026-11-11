//! Anthropic Messages API backend (real Claude conversations).
//!
//! Streams a response from `POST https://api.anthropic.com/v1/messages`
//! with `stream: true`. The wire format is server-sent events; this
//! module parses them manually rather than pulling a dedicated SSE
//! crate — the protocol is short (line-prefixed `data:` events
//! separated by blank lines) and a hand-rolled parser keeps the
//! dependency surface honest.
//!
//! Events handled:
//! - `content_block_delta` with `delta.type = "text_delta"` →
//!   `StreamMsg::Chunk(text)`.
//! - `message_start` / `message_delta` → `StreamMsg::Usage`, and the
//!   turn's `stop_reason` → `StreamMsg::Stopped`.
//! - `message_stop` → `StreamMsg::Complete`.
//! - **Tool calls**, which arrive spread over three events —
//!   `content_block_start` (the `id`/`name`), a run of
//!   `input_json_delta`s (the arguments, as JSON text fragments that
//!   are only valid concatenated), and `content_block_stop`. An
//!   `Assembler` joins them into ONE `StreamMsg::ToolUse` block, so
//!   the consumer gates a whole request rather than fragments.
//!   Arguments that don't parse drop the call: one we cannot read is
//!   one we must not run.
//! - **Thinking**, the same way: `thinking_delta`s →
//!   `StreamMsg::ThinkingChunk` for the screen, and at the block's stop
//!   the whole block, signature and all → `StreamMsg::Thought`, to send
//!   back unchanged; `redacted_thinking` → `Thought` too. The blocks the
//!   API dropped from the request (`message_start`'s
//!   `input_transformations`) → `StreamMsg::ThinkingDropped`.
//! - Network errors, non-2xx HTTP responses, malformed bytes →
//!   `StreamMsg::Error(BackendError)`, categorized so the consumer
//!   can branch (auth vs. rate-limit vs. transport).
//!
//! Everything else (`ping`, a text block's start/stop) is silently
//! ignored — not user-visible text the consumer needs.
//!
//! Model defaults to Sonnet 5.5 ([`DEFAULT_MODEL`]). The constructor
//! accepts an override; a session's own model (`:model`) is what a request
//! carries.
//!
//! **Prompt caching** ([`CacheConfig`]) is a property of the backend, not of a
//! request: every turn carries the same top-level `cache_control`, which the
//! API pins to the last cacheable block. Each request therefore leaves an entry
//! at its own tail, and the next one — or a branch taken from any earlier turn
//! within the entry's lifetime — reads that shared prefix at a tenth of the
//! price rather than sending it again. A prompt shorter than the model's
//! minimum cacheable length is processed uncached with no error and no
//! surcharge, so a short session neither gains nor loses by this being on.

use std::time::Duration;

use anyhow::Context;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tracing::{Instrument, debug, info, warn};

use super::{Backend, SendRequest, ToolDef};
use crate::config::CacheConfig;
use crate::{BackendError, Message, Secret, StopReason, StreamMsg, Usage};

/// Cap on a returned error body before it lands in user-facing text.
const MAX_ERROR_BODY: usize = 200;

/// Default model — what a session starts on when `[tui] model` is empty
/// (Steven, 10-06: "Let's set Sonnet to the default"). It was Haiku 4.5,
/// which takes no effort and retires no sooner than 2026-10-15.
pub const DEFAULT_MODEL: &str = "claude-sonnet-5-5";

/// Anthropic API version pinned in the request header. The Messages
/// API is stable; bumping this requires updating the wire format
/// expected below.
const API_VERSION: &str = "2023-06-01";

/// Endpoint. Hard-coded because there is only one — provider URL
/// overrides land when we add additional backends.
const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";

/// **How long a reply may go quiet before its connection is taken as
/// stalled** — per read, reset by every chunk that arrives, so a reply that
/// keeps coming is never cut however long it runs. Generous, because the
/// first byte can wait on the model reading a long prompt at a high effort.
///
/// It was a TOTAL deadline (`ClientBuilder::timeout`: "until the response
/// body has finished") of ten minutes, under a doc calling it a stall
/// detector — so a long reply still streaming at minute ten was killed, and
/// a `max`-effort reply on a 128k-output model can run that long (10-04).
const STALL_TIMEOUT: Duration = Duration::from_secs(300);

/// How long reaching the API may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

// The reply-length cap used to live here as `MAX_OUTPUT_TOKENS = 4096`, with
// the rationale "so a test key with a small budget cannot accidentally be
// drained by a runaway turn". It shipped in M1 and was never questioned again —
// including by me, twice this week, editing the lines around it.
//
// It guillotined all seven turns of a live demo. Four of them returned NOTHING
// while billing the full 4,096 output tokens, because whatever filled those
// tokens was not text and this module dropped it. A spend guard belongs on
// spend and it belongs where you can see it; it does not belong on the length
// of an answer, silently. The ceiling is now the model's own
// (`models::max_output_tokens`), lowerable by config.

/// Cap on the SSE reassembly buffer. A single `text_delta` event is a
/// few hundred bytes; 1 MiB only triggers if the server (or a broken
/// middlebox) streams data with no `\n\n` event boundary — without the
/// cap that would grow the buffer until the process OOMs.
const MAX_SSE_BUFFER: usize = 1024 * 1024;

pub struct AnthropicApi {
    api_key: Secret,
    model: String,
    /// Behind a lock so [`Backend::set_cache`] can change it for the next
    /// request while turns already streaming keep what they were sent with.
    cache: std::sync::Mutex<CacheConfig>,
    /// Where requests go. Always [`ENDPOINT`] in a running program; a test
    /// points it at a local socket so the bytes we actually send can be read
    /// back. Serialising a [`RequestBody`] by hand proves what the struct
    /// does, not what the four hops from config to socket do with it.
    endpoint: String,
    http: reqwest::Client,
}

impl AnthropicApi {
    /// `api_key` is wrapped in `Secret` so it cannot be logged or
    /// `Debug`-printed by accident. The HTTP client is constructed
    /// once per backend instance so connection pooling works across
    /// turns.
    ///
    /// The cache policy is a **constructor argument, not a builder step**, and
    /// not a [`SendRequest`] field either. Every request wants the same answer,
    /// and the rolling trail only works if *every* request leaves an entry: a
    /// knob a call site can forget is a knob that produces a cache with holes
    /// in it, silently and at full price. One place decides, and the three
    /// request-building sites in the TUI cannot disagree with it — a changed
    /// `[cache]` reaches that one place through [`Backend::set_cache`].
    pub fn new(
        api_key: impl Into<Secret>,
        model: impl Into<String>,
        cache: CacheConfig,
    ) -> anyhow::Result<Self> {
        let http = http_client()?;
        Ok(Self {
            api_key: api_key.into(),
            model: model.into(),
            cache: std::sync::Mutex::new(cache),
            endpoint: ENDPOINT.to_string(),
            http,
        })
    }

    /// Point this backend at another `/v1/messages` URL. Test-only: the
    /// provider-URL override a second backend would want is a config decision
    /// nobody has asked for yet, and a knob with no consumer is a knob that
    /// drifts.
    #[cfg(test)]
    fn with_endpoint(mut self, url: impl Into<String>) -> Self {
        self.endpoint = url.into();
        self
    }
}

/// The HTTP client every request goes through.
///
/// **Extracted so the redirect policy can be tested rather than asserted.** The
/// paragraph below is correct and was, until now, the only thing holding the
/// control in place — mutating `Policy::none()` to `Policy::limited(10)` left
/// the whole suite green (Reviewer E). In this codebase a comment good enough
/// to persuade you is a comment good enough to stop you counting, and this one
/// guards a credential.
fn http_client() -> anyhow::Result<reqwest::Client> {
    client_stalling_after(STALL_TIMEOUT)
}

/// **A transport failure, naming the bound that fired** when one did.
/// reqwest's own text names only the kind — a stalled reply read "network
/// error: request or response body error", nothing to tell it from a dropped
/// connection or to say what to do (Reviewer, 10-04).
fn transport(e: reqwest::Error) -> BackendError {
    BackendError::Transport(if e.is_timeout() && e.is_connect() {
        format!(
            "could not reach the API within {} s",
            CONNECT_TIMEOUT.as_secs()
        )
    } else if e.is_timeout() {
        format!(
            "no data from the API for {} s — the reply was taken as stalled",
            STALL_TIMEOUT.as_secs()
        )
    } else {
        e.to_string()
    })
}

/// [`http_client`], taking a reply as stalled after `stall` of silence — a
/// parameter so a test can measure the rule at a scale it can wait for.
fn client_stalling_after(stall: Duration) -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(stall)
        // Disable redirect following. The Messages API never
        // redirects, and `x-api-key` is a *custom* header that
        // reqwest would forward across a cross-host redirect
        // (it only strips the well-known `Authorization` /
        // `Cookie` on redirect). Refusing redirects removes any
        // path by which the key could be replayed to another host.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("build reqwest client")
}

/// One outgoing turn with every override already resolved — the backend's
/// defaults folded under the request's, the config policy turned into the wire
/// object.
///
/// A struct because the alternative is eight positional arguments to
/// [`run_turn`], which is where a `model` and a `max_tokens` eventually get
/// swapped by someone adding a ninth. It also draws the line the other way
/// round from [`SendRequest`]: that one is what a *caller* may ask for, this is
/// what actually goes on the wire.
struct Outgoing {
    endpoint: String,
    api_key: Secret,
    model: String,
    max_tokens: u32,
    system: Option<String>,
    cache: Option<CacheControl>,
    messages: Vec<Message>,
    tools: Vec<ToolDef>,
    effort: Option<crate::Effort>,
    thinking: Option<ThinkingParam>,
}

impl Backend for AnthropicApi {
    fn send_message(&self, req: SendRequest, tx: UnboundedSender<StreamMsg>) -> JoinHandle<()> {
        let http = self.http.clone();
        let model = req.model.clone().unwrap_or_else(|| self.model.clone());
        let turn = Outgoing {
            endpoint: self.endpoint.clone(),
            api_key: self.api_key.clone(),
            thinking: ThinkingParam::for_model(&model, req.thinking),
            // Per-turn overrides fall back to the instance defaults. The
            // reply cap falls back to the MODEL's own ceiling, computed from
            // the model actually resolved above — not from `self.model`, which
            // a `:model` or `-m` override has just replaced.
            model,
            max_tokens: req.max_tokens.unwrap_or_else(|| {
                super::models::max_output_tokens(req.model.as_deref().unwrap_or(&self.model))
            }),
            // A poisoned lock still holds a whole `CacheConfig` (it is `Copy`,
            // written in one assignment), so its value is used, not dropped.
            cache: CacheControl::from_config(
                *self
                    .cache
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            ),
            system: req.system,
            // As the model reads them: a context block is text on the wire,
            // which rejects a type it does not know (`Message::wire`).
            messages: req.messages.iter().map(|m| m.wire().into_owned()).collect(),
            tools: req.tools,
            effort: req.effort,
        };

        // **The caller's session span, carried into the task.** `Outgoing`
        // deliberately holds no episode id — this crate has no business knowing
        // about sessions — so the attribution rides the span instead of a
        // field, and nothing in the public API changed to get it.
        //
        // `Span::current()` is read in this synchronous frame: a span entered
        // inside the async block would not survive the first `.await`.
        //
        // It is captured at SPAWN, which is the right instant rather than a
        // stale one. These lines describe *this request*, built from the
        // episode as it was when it was sent; if the caller's buffer is later
        // repointed at another episode, that does not retroactively change
        // whose request this was.
        tokio::spawn(
            async move {
                if let Err(err) = run_turn(http, turn, &tx).await {
                    // The categorized error renders itself concisely via
                    // Display; full detail (incl. the variant) goes to the
                    // log file. The key is never part of either.
                    warn!(error = %err, "anthropic api turn failed");
                    let _ = tx.send(StreamMsg::Error(err));
                }
            }
            .instrument(tracing::Span::current()),
        )
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn set_cache(&self, cache: CacheConfig) {
        *self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = cache;
    }
}

async fn run_turn(
    http: reqwest::Client,
    turn: Outgoing,
    tx: &UnboundedSender<StreamMsg>,
) -> Result<(), BackendError> {
    let request = RequestBody {
        model: &turn.model,
        max_tokens: turn.max_tokens,
        stream: true,
        cache_control: turn.cache,
        system: turn.system.as_deref(),
        messages: &turn.messages,
        tools: &turn.tools,
        output_config: turn.effort.map(|effort| OutputConfig { effort }),
        thinking: turn.thinking,
    };
    let betas = turn.thinking.map(ThinkingParam::betas).unwrap_or_default();

    // `tools` and `max_tokens` are on this line because diagnosing the demo
    // needed both and the log could answer neither — it took the JSONL store to
    // find out that a tool was even offered ([[feedback_no_silent_failure]]).
    // The effort, the betas and the thinking sent back for the same reason:
    // "the API dropped 1 thinking block" is traced only beside how many went
    // (Steven, 10-31: "give users the ability to trace these types of things
    // themselves").
    let thinking_sent = turn
        .messages
        .iter()
        .flat_map(|m| &m.blocks)
        .filter(|b| {
            matches!(
                b,
                crate::Block::Thinking { .. } | crate::Block::RedactedThinking { .. }
            )
        })
        .count();
    info!(
        model = %turn.model,
        turns = turn.messages.len(),
        cache = turn.cache.is_some(),
        max_tokens = turn.max_tokens,
        tools = turn.tools.len(),
        system = turn.system.is_some(),
        effort = ?turn.effort,
        thinking = ?turn.thinking,
        betas = %betas.join(","),
        thinking_sent,
        "sending anthropic messages request",
    );

    let mut post = http
        .post(&turn.endpoint)
        .header("x-api-key", turn.api_key.expose())
        .header("anthropic-version", API_VERSION);
    if !betas.is_empty() {
        post = post.header("anthropic-beta", betas.join(","));
    }
    let response = post.json(&request).send().await.map_err(transport)?;

    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unreadable body>".to_string());
        return Err(api_failure(status.as_u16(), &body));
    }

    // **Bytes, not a String.** `bytes_stream` yields whatever the network hands
    // over, and a TCP segment boundary respects nothing — an em dash, a curly
    // quote or an emoji straddling one is three bytes split across two chunks.
    // Decoding each chunk on arrival turned that into `BackendError::Decode` and
    // ended the turn: the longer the reply, the more boundaries, the likelier
    // it was to die. Reassemble first, decode whole events after.
    let mut buffer = SseBuffer::default();
    let mut stream = response.bytes_stream();
    // Whether `message_stop` arrived. The consumer's turn lifecycle
    // hinges on a terminal message (`Complete` / `Error`): if the
    // connection just ends without one, we must synthesize an error
    // or the TUI's single-turn gate stays closed forever.
    let mut completed = false;
    // Carries a `tool_use` or thinking block across the events that build it
    // (see `Assembler`); inert for a turn of text alone.
    let mut open = Assembler::default();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(transport)?;
        buffer.push(&chunk)?;
        // SSE events are separated by blank lines (`\n\n`). Drain each
        // complete event before waiting for more bytes. The separator is ASCII,
        // so finding it in bytes is exact — and a complete event is a complete
        // UTF-8 sequence, which is what makes the decode below safe where a
        // per-chunk one was not.
        while let Some(event) = buffer.next_event() {
            let event = event?;
            for msg in parse_sse_event(&event, &mut open) {
                // An in-band `error` ends the turn as the HTTP error it
                // stands for would: returned, through the one door that
                // sends and logs it — not followed by "ended before
                // message_stop", a second ending for one turn.
                if let StreamMsg::Error(err) = msg {
                    return Err(err);
                }
                completed |= msg == StreamMsg::Complete;
                if tx.send(msg).is_err() {
                    // Receiver dropped — turn aborted; stop streaming.
                    return Ok(());
                }
            }
        }
    }
    // **A block still open when the bytes stop is a block we are about to
    // destroy.** The assembler held a half-written `tool_use` and was simply
    // dropped here: the model asked to run a command, the reply was cut off
    // mid-JSON, and the request ceased to exist with no error, no warning and
    // no transcript entry. In the demo that produced four turns that billed
    // 4,096 output tokens and rendered nothing at all. Whatever else is true,
    // a failure must reach a human ([[feedback_visible_refusals]]) — and the
    // truncation that caused it is named here rather than left to be inferred.
    if let Some(lost) = open.abandoned() {
        warn!(
            tool = %lost.name,
            bytes = lost.bytes,
            "the reply was cut off part-way through a tool call; it cannot be run"
        );
        let _ = tx.send(StreamMsg::Error(BackendError::Truncated {
            tool: lost.name,
            bytes: lost.bytes,
        }));
        return Ok(());
    }
    if completed {
        Ok(())
    } else {
        Err(BackendError::Transport(
            "stream ended before message_stop".to_string(),
        ))
    }
}

/// Reassembles whole SSE events out of raw network bytes.
///
/// **The point is that a chunk boundary is not a character boundary.**
/// `bytes_stream` yields TCP segments, which split wherever the network felt
/// like it: an em dash is three bytes and a boundary can fall between any two
/// of them. `run_turn` used to decode each chunk on arrival, so that returned
/// `BackendError::Decode` and ended the turn — and the longer the reply, the
/// more boundaries it had to survive.
///
/// A type rather than two loose functions inside the stream loop, because the
/// invariant is only checkable if the split points are: as a unit it can be fed
/// a payload cut at *every* offset, which is the only way to be sure
/// ([[feedback_fuzz_the_boundary]]). Through the network it cannot be tested at
/// all — reqwest coalesces, so a server writing one byte at a time still
/// arrives in one piece, and a test built that way passes whether the bug is
/// there or not.
#[derive(Default)]
struct SseBuffer {
    bytes: Vec<u8>,
    /// **Where a resumed scan must start** — not "how far we looked".
    ///
    /// **Without a cursor at all the scan is O(n²) in the buffered bytes**,
    /// because it restarts at byte 0 on every network read and there is one
    /// read per TCP segment. Reviewer B reproduced it: byte-at-a-time delivery
    /// of a single large event pegged a core for over 160 seconds and had still
    /// not reached the 1 MiB cap. `run_turn` is a spawned task with no `.await`
    /// inside the loop, so that is a pinned worker thread — the same "it froze
    /// on longer replies" failure this session fixed on the render side,
    /// reopened on the parse side by the fix for the character-split bug.
    ///
    /// **Stated as a resume point, so there is no overlap to size.** It was
    /// "how far we looked" plus a constant of look-back, and the two were
    /// redundant: with the constant at 2, deliberately parking the cursor one
    /// byte *past* an undecidable candidate still passed every test, and so did
    /// shrinking the constant to 1. Two mechanisms covering one invariant means
    /// neither can be measured, and slack in a guard reads as a guard
    /// (`[[feedback_mutation_check_guards]]`).
    ///
    /// One rule instead: [`next_event`](Self::next_event) leaves this on the
    /// first byte of an **undecidable candidate**, or at `bytes.len()` when it
    /// walked off the end without one — and *that* is exact, because a `\n` it
    /// walked past was either ruled out for good or was the candidate it parked
    /// on. Reset on every drain, since the bytes it counted are gone.
    resume_at: usize,
    /// Total byte-pairs the separator scan has looked at, over this buffer's
    /// whole life.
    ///
    /// **The property is linear TOTAL WORK, and this measures it directly.**
    /// The first version of the test timed a large fill instead, and the
    /// quadratic mutation completed inside the timeout and passed — a margin
    /// guessed rather than measured (`[[feedback_mutation_check_guards]]`).
    /// Counting the comparisons removes the margin: quadratic fails at any
    /// size, so the test can stay small and fast.
    #[cfg(test)]
    compared: usize,
}

impl SseBuffer {
    /// Take more bytes off the wire. `Err` only when the peer has sent more
    /// than [`MAX_SSE_BUFFER`] with no event boundary in it.
    fn push(&mut self, chunk: &[u8]) -> Result<(), BackendError> {
        // Checked BEFORE the copy as well as after. The cap bounds accumulation
        // across many small reads; it did nothing about one oversized read,
        // which allocated in full and was then rejected (Reviewer B, who pushed
        // 50 MiB through it).
        if self.bytes.len().saturating_add(chunk.len()) > MAX_SSE_BUFFER {
            return Err(BackendError::Decode(format!(
                "sse event exceeded {MAX_SSE_BUFFER} bytes without an event boundary"
            )));
        }
        self.bytes.extend_from_slice(chunk);
        Ok(())
    }

    /// The next complete event, or `None` while one is still arriving.
    ///
    /// Events are separated by a **blank line**, and a line may end `\n` or
    /// `\r\n` — so the separator is `\n\n` or `\r\n\r\n`. Anthropic sends the
    /// first; this looked for *only* the first, so a proxy that normalises line
    /// endings (and normalising them is exactly what such a thing is for) made
    /// the stream unreadable: no separator is ever found, the buffer fills to
    /// [`MAX_SSE_BUFFER`], and the user is told their event "exceeded 1048576
    /// bytes without an event boundary" — true, useless, and pointing at the
    /// wrong thing entirely (Reviewer B). Recognising both costs one branch.
    ///
    /// The event keeps whatever `\r` precedes its final newline; `str::lines`
    /// and the `trim` in [`parse_sse_event`] both strip it, so the parse side
    /// needed nothing.
    ///
    /// The separators are ASCII and UTF-8 continuation bytes are all `>= 0x80`,
    /// so a byte scan for them can never land inside a character — which is
    /// what makes the decode below safe where a per-chunk one was not. A whole
    /// event that is still not valid UTF-8 is a real protocol error and is
    /// reported as one.
    fn next_event(&mut self) -> Option<Result<String, BackendError>> {
        // Resume exactly where the last scan said to — see `resume_at`. No
        // look-back: a separator split across two reads is found because the
        // scan PARKED on it, not because the next scan re-examines bytes.
        let from = self.resume_at;
        #[cfg(test)]
        {
            self.compared += self.bytes.len().saturating_sub(from);
        }
        let mut i = from;
        let consumed = loop {
            let Some(&byte) = self.bytes.get(i) else {
                // Walked off the end with nothing undecidable behind us: every
                // `\n` back there had a follower that ruled it out, so no
                // separator can begin before here.
                self.resume_at = self.bytes.len();
                return None;
            };
            if byte == b'\n' {
                match self.bytes.get(i + 1) {
                    // A blank line, LF-terminated.
                    Some(b'\n') => break i + 2,
                    // …or CRLF-terminated. `\r\n\r\n` reaches here on its
                    // SECOND `\n`, which is why two bytes of overlap are
                    // enough for a four-byte separator.
                    Some(b'\r') => match self.bytes.get(i + 2) {
                        Some(b'\n') => break i + 3,
                        Some(_) => {}
                        // Undecidable until more arrives: park ON this `\n`, so
                        // the resumed scan sees the whole candidate. This is
                        // the only thing making a straddling separator work.
                        None => {
                            self.resume_at = i;
                            return None;
                        }
                    },
                    Some(_) => {}
                    // Likewise: a trailing `\n` may begin either separator.
                    None => {
                        self.resume_at = i;
                        return None;
                    }
                }
            }
            i += 1;
        };
        let event: Vec<u8> = self.bytes.drain(..consumed).collect();
        // The bytes the cursor was addressing are gone with the drain.
        self.resume_at = 0;
        Some(String::from_utf8(event).map_err(|e| BackendError::Decode(e.to_string())))
    }
}

/// Translate a raw SSE event (a multi-line block ending in `\n\n`) into zero
/// or more `StreamMsg`s — `message_delta` carries BOTH the turn's output
/// usage and its `stop_reason`, so one wire event can yield two messages.
/// Ignored event types yield none.
///
/// Reads the `data:` line; the `event:` line is ignored because the
/// `type` field inside the JSON payload is authoritative and present
/// on every event. (Anthropic emits a single `data:` line per event;
/// the SSE spec permits multiple, which we'd need to concatenate, but
/// that case does not arise against this API.)
/// Assembles a content block that arrives in pieces — a `tool_use`, or a
/// thinking block — across the events that carry it.
///
/// Text arrives whole in each delta, but a tool call is spread over
/// `content_block_start` (the `id`/`name`), a run of `input_json_delta`s (the
/// arguments, as JSON *text fragments* that are only valid once concatenated),
/// and `content_block_stop`; a thinking block over its start, `thinking_delta`s
/// and a `signature_delta`, and is kept only whole, signature and all.
/// Keeping that state in one small self-contained struct lets
/// [`parse_sse_event`] stay a pure function of (event, accumulator) and keeps
/// the partial handling in exactly one place.
#[derive(Default)]
struct Assembler {
    /// The block being built, if one that arrives in pieces is open.
    pending: Option<Pending>,
}

/// The block [`Assembler`] holds open.
enum Pending {
    Tool {
        id: String,
        name: String,
        /// Concatenated `partial_json` fragments — parsed only at block stop.
        json: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    /// Arrives whole at its start; kept to its stop, as every block is.
    Redacted {
        data: String,
    },
}

/// A `tool_use` block that the stream ended in the middle of — what it was
/// going to call, and how far it got.
pub(super) struct Abandoned {
    pub(super) name: String,
    pub(super) bytes: usize,
}

impl Assembler {
    /// The tool call left half-written when the bytes stopped, if there is
    /// one. A thinking block cut off is not a call: without its signature it
    /// cannot be sent back, and the reply's ending says what was cut.
    ///
    /// Consuming, because the only caller is the end of the stream and there is
    /// nothing left to assemble — and because a `take` cannot be mistaken for a
    /// peek that leaves the state around to be reported twice.
    fn abandoned(&mut self) -> Option<Abandoned> {
        match self.pending.take()? {
            Pending::Tool { name, json, .. } => Some(Abandoned {
                name,
                bytes: json.len(),
            }),
            Pending::Thinking { .. } | Pending::Redacted { .. } => None,
        }
    }

    /// A block that arrives in pieces opened.
    fn begin(&mut self, pending: Pending) {
        self.pending = Some(pending);
    }

    /// A fragment of the open tool call's input JSON arrived. Ignored when no
    /// `tool_use` block is open (a text block's deltas take the other arm).
    fn push_json(&mut self, fragment: &str) {
        if let Some(Pending::Tool { json, .. }) = self.pending.as_mut() {
            json.push_str(fragment);
        }
    }

    /// Thinking text arrived, for the open thinking block.
    fn push_thinking(&mut self, text: &str) {
        if let Some(Pending::Thinking { thinking, .. }) = self.pending.as_mut() {
            thinking.push_str(text);
        }
    }

    /// The open thinking block's signature arrived.
    fn push_signature(&mut self, text: &str) {
        if let Some(Pending::Thinking { signature, .. }) = self.pending.as_mut() {
            signature.push_str(text);
        }
    }

    /// The open block closed: the finished block, as the stream message that
    /// carries it. A tool call's JSON is parsed here — an empty fragment run
    /// means "no arguments" (`{}`); malformed JSON yields `None` and is dropped
    /// with a warning rather than killing the turn — a tool call we cannot
    /// read is one we must not run.
    fn finish(&mut self) -> Option<StreamMsg> {
        match self.pending.take()? {
            Pending::Tool { id, name, json } => {
                let text = if json.trim().is_empty() {
                    "{}"
                } else {
                    json.as_str()
                };
                match serde_json::from_str(text) {
                    Ok(input) => Some(StreamMsg::ToolUse(crate::Block::ToolUse {
                        id,
                        name,
                        input,
                    })),
                    Err(err) => {
                        warn!(err = %err, tool = %name, "unparseable tool_use input; dropping the call");
                        None
                    }
                }
            }
            // Placed by the conversation, which knows the reply's text so far.
            Pending::Thinking {
                thinking,
                signature,
            } => Some(StreamMsg::Thought(crate::Block::Thinking {
                thinking,
                signature,
                at: 0,
            })),
            Pending::Redacted { data } => {
                Some(StreamMsg::Thought(crate::Block::RedactedThinking {
                    data,
                    at: 0,
                }))
            }
        }
    }
}

fn parse_sse_event(raw: &str, open: &mut Assembler) -> Vec<StreamMsg> {
    let Some(data) = raw
        .lines()
        .find_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
    else {
        return Vec::new();
    };

    let event: SseEvent = match serde_json::from_str(data) {
        Ok(e) => e,
        Err(err) => {
            // **The payload is not logged.** `data` is model output — the
            // assistant's text, or a tool call's whole argument object — and a
            // log file is durable and gets attached to bug reports. This is the
            // one content site in the crate that CANNOT scrub its way out:
            // `Outgoing` carries no alias table, so there is no session
            // substitution to map a lent value back through.
            //
            // What a parse failure actually needs is where it broke and how
            // much arrived, both of which `serde_json` gives positionally. The
            // bytes themselves are recoverable from the transcript in the
            // store, which is the sink that is supposed to hold them.
            debug!(
                err = %err,
                bytes = data.len(),
                "sse parse failed",
            );
            return Vec::new();
        }
    };

    match event {
        SseEvent::ContentBlockDelta { delta } => match delta {
            DeltaKind::TextDelta { text } => vec![StreamMsg::Chunk(text)],
            // Arguments for the open `tool_use` block, in fragments that are only
            // valid JSON once concatenated — accumulate, don't parse yet.
            DeltaKind::InputJsonDelta { partial_json } => {
                open.push_json(&partial_json);
                Vec::new()
            }
            // Thinking text, kept for the block and shown as it comes.
            DeltaKind::ThinkingDelta { thinking } => {
                open.push_thinking(&thinking);
                match thinking.is_empty() {
                    true => Vec::new(),
                    false => vec![StreamMsg::ThinkingChunk(thinking)],
                }
            }
            DeltaKind::SignatureDelta { signature } => {
                open.push_signature(&signature);
                Vec::new()
            }
            DeltaKind::Other => Vec::new(),
        },
        // A content block opened. A text block's content arrives whole in its
        // deltas; the others are assembled to their stop.
        SseEvent::ContentBlockStart { content_block } => {
            match content_block {
                WireBlockStart::ToolUse { id, name } => open.begin(Pending::Tool {
                    id,
                    name,
                    json: String::new(),
                }),
                WireBlockStart::Thinking {
                    thinking,
                    signature,
                } => open.begin(Pending::Thinking {
                    thinking,
                    signature,
                }),
                WireBlockStart::RedactedThinking { data } => open.begin(Pending::Redacted { data }),
                WireBlockStart::Other => {}
            }
            Vec::new()
        }
        // A content block closed: emit the assembled block, if this was one.
        SseEvent::ContentBlockStop => open.finish().into_iter().collect(),
        // `message_start` carries the prompt's input token count (output is ~0
        // here); `message_delta` carries the turn's cumulative output count
        // AND why the turn is stopping. Usage readings merge per-field in the
        // consumer; the stop reason lands in the turn's provenance.
        // `message_start` also says which thinking blocks the API dropped from
        // the request (`input_transformations`, with the binding beta).
        SseEvent::MessageStart { message } => {
            let dropped: Vec<crate::Dropped> = (message.input_transformations.iter())
                .filter(|t| t.kind == "thinking_dropped")
                .map(|t| crate::Dropped::from_wire(&t.reason))
                .collect();
            let mut out = vec![StreamMsg::Usage(message.usage.into())];
            if !dropped.is_empty() {
                out.push(StreamMsg::ThinkingDropped(dropped));
            }
            out
        }
        SseEvent::MessageDelta { delta, usage } => {
            let mut out = vec![StreamMsg::Usage(usage.into())];
            if let Some(reason) = delta.stop_reason {
                out.push(StreamMsg::Stopped(StopReason::from_wire(&reason)));
            }
            out
        }
        SseEvent::MessageStop => vec![StreamMsg::Complete],
        SseEvent::Error { error } => {
            vec![StreamMsg::Error(api_failure(stands_for(&error.kind), data))]
        }
        SseEvent::Other => Vec::new(),
    }
}

/// **The status an in-band `error` event stands for** — the API's streaming
/// form of an HTTP error, after a 200 (the claude-api reference's table, read
/// 10-26); one it does not list is the 500 it would be. The event fell to
/// `Other`, and the turn was said to have "ended before message_stop" — an
/// overload reported as a broken connection (review stop 12, T9 5).
fn stands_for(kind: &str) -> u16 {
    match kind {
        "invalid_request_error" => 400,
        "authentication_error" => 401,
        "permission_error" => 403,
        "not_found_error" => 404,
        "request_too_large" => 413,
        "rate_limit_error" => 429,
        "overloaded_error" => 529,
        _ => 500,
    }
}

/// **One failure, however the API reports it** — a status with a body, or an
/// in-band `error` event ([`stands_for`]) with its data. 401 and 429 are the
/// two a user answers differently (the key; waiting). Any other is the status
/// and what the server said, verbatim but made safe for the log and the
/// screen: control characters dropped so it cannot forge log lines or smuggle
/// escape sequences (the TUI sanitizes again at ingest; the log writer does
/// not), and cut to [`MAX_ERROR_BODY`].
fn api_failure(status: u16, said: &str) -> BackendError {
    match status {
        401 => BackendError::Auth,
        429 => BackendError::RateLimited,
        status => BackendError::Api {
            status,
            message: said
                .chars()
                .filter(|c| !c.is_control())
                .take(MAX_ERROR_BODY)
                .collect(),
        },
    }
}

#[derive(Serialize)]
struct RequestBody<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    /// The **rolling prompt-cache breakpoint**, or nothing when caching is off.
    ///
    /// Top-level rather than stamped onto a content block, which is the API's
    /// own "automatic caching" form: it applies the breakpoint to the last
    /// cacheable block and moves it forward by itself as the conversation
    /// grows. That is worth more than the control we give up. The alternative
    /// was to serialise each [`Message`] to JSON and patch `cache_control` into
    /// the tail block — a second, hand-written copy of a wire shape that
    /// `Message`'s own serde already owns, and the exact shape that rots
    /// (`[[feedback_rules_rot_by_retyping]]`), in exchange for a placement the
    /// server picks correctly anyway.
    ///
    /// Marking the tail is what leaves the **trail**: each request writes an
    /// entry at its own end, so every turn boundary ends up holding one, and a
    /// branch taken from any turn still inside the TTL reads the shared prefix
    /// instead of re-paying for it. The API walks back up to 20 content blocks
    /// from a breakpoint looking for an earlier entry, and one exchange is two
    /// to four blocks, so consecutive requests stay comfortably inside it.
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
    /// The system prompt. Omitted entirely when there is none, so a request
    /// without one is byte-identical to what this program sent before the field
    /// existed. The API orders the cached prefix `tools` → `system` →
    /// `messages`, so a stable system prompt sits inside the cache and costs
    /// nothing after the first turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    /// Serialised directly: `Message`'s own serde emits the exact Messages-API
    /// shape (a bare `content` string, or the typed block array when a turn
    /// carries `tool_use`/`tool_result`), so no separate wire type is needed.
    messages: &'a [Message],
    /// Tools offered this turn. `ToolDef` serialises to the Anthropic `tools`
    /// entry shape; omitted entirely when empty so a plain turn's body is
    /// unchanged.
    #[serde(skip_serializing_if = "no_tools")]
    tools: &'a [ToolDef],
    /// `{"effort": …}`, when an effort was asked for. Omitted otherwise, so
    /// the model works at its own default and the body is what it was before
    /// the field existed. Top-level, not per message: changing it between
    /// requests restarts the prompt cache (the API's effort page), which is
    /// why `:effort` says so.
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<OutputConfig>,
    /// What thinking is asked for, by the model's category
    /// ([`ThinkingParam::for_model`]); omitted where it asks for none.
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingParam>,
}

/// The beta under which `block_binding` is read and `input_transformations`
/// reported. The field without it is a 400.
const BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";
/// The beta `display: "updates"` needs.
const UPDATES_BETA: &str = "thinking-display-updates-2026-08-18";

/// **The API's `thinking` object, as a model's category asks for it**
/// (`models::thinking_kind`, `docs/DesignDocs/navigation.md` §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ThinkingParam {
    /// 4.6 and later: what it shows, and that a block the history no longer
    /// binds is dropped and said, never a 400 (NQ11's defence; adaptive is
    /// the one type `block_binding` works with).
    Adaptive {
        display: crate::ThinkingDisplay,
        block_binding: BlockBinding,
    },
    /// 4.5 and earlier, when a budget is set.
    Enabled { budget_tokens: u32 },
}

/// `thinking.block_binding`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct BlockBinding {
    prefix_mismatch_behavior: &'static str,
}

impl ThinkingParam {
    /// What `model` is asked, for `ask`: nothing for a model with no
    /// generation this build reads, nor for a budgeted one with no budget.
    fn for_model(model: &str, ask: super::ThinkingAsk) -> Option<ThinkingParam> {
        match super::models::thinking_kind(model)? {
            super::models::ThinkingKind::Adaptive => Some(ThinkingParam::Adaptive {
                display: ask.display,
                block_binding: BlockBinding {
                    prefix_mismatch_behavior: "drop_block",
                },
            }),
            super::models::ThinkingKind::Budget => {
                (ask.budget > 0).then_some(ThinkingParam::Enabled {
                    budget_tokens: ask.budget,
                })
            }
        }
    }

    /// The betas it needs, for `anthropic-beta`.
    fn betas(self) -> Vec<&'static str> {
        match self {
            ThinkingParam::Adaptive { display, .. } => {
                let mut betas = vec![BINDING_BETA];
                if display == crate::ThinkingDisplay::Updates {
                    betas.push(UPDATES_BETA);
                }
                betas
            }
            ThinkingParam::Enabled { .. } => Vec::new(),
        }
    }
}

/// The API's `output_config` object — today, only `effort`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct OutputConfig {
    effort: crate::Effort,
}

/// `skip_serializing_if` predicate for the borrowed `tools` slice — serde hands
/// the predicate a reference to the field, i.e. `&&[ToolDef]`.
fn no_tools(tools: &&[ToolDef]) -> bool {
    tools.is_empty()
}

/// The API's `cache_control` object: `{"type": "ephemeral"}`, plus `ttl` when
/// the user asked for the hour-long entry (the five-minute default is the
/// *absence* of `ttl`, not a value).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct CacheControl {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    ttl: Option<&'static str>,
}

impl CacheControl {
    /// `None` when caching is off — the field then does not appear at all, so a
    /// request is byte-identical to one from before this feature existed.
    fn from_config(cache: CacheConfig) -> Option<Self> {
        cache.enabled.then(|| Self {
            kind: "ephemeral",
            ttl: cache.ttl.wire(),
        })
    }
}

/// Subset of the streaming SSE event types we care about. The catch-
/// all `Other` keeps the parser tolerant to event types that exist
/// (e.g. `ping`, `message_start`) or that the API adds in the
/// future; we just ignore them.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SseEvent {
    // The `index` field exists on the wire (which content block this
    // delta belongs to) but M1 streams a single text block, so we
    // don't deserialize it. Internally-tagged enums ignore unknown
    // fields, so omitting it is safe.
    ContentBlockDelta {
        delta: DeltaKind,
    },
    /// A content block opened. Its `content_block` says which kind; we track
    /// `tool_use` (to collect its id/name) and ignore the rest.
    ContentBlockStart {
        content_block: WireBlockStart,
    },
    /// A content block closed — the point a `tool_use` block is complete.
    ContentBlockStop,
    /// First event of the stream — its `message.usage` has the input tokens.
    MessageStart {
        message: WireMessageStart,
    },
    /// Near the end — its `usage` has the turn's output tokens and its
    /// `delta` carries the turn's `stop_reason`.
    MessageDelta {
        #[serde(default)]
        delta: WireMessageDeltaBody,
        usage: WireUsage,
    },
    MessageStop,
    /// The API's in-band error (`{"type":"error","error":{"type":…,
    /// "message":…}}`), before `message_start` or mid-stream: the turn ends
    /// on it ([`api_failure`]). Known by its tag alone — an event that fails
    /// to parse is dropped, so a field it lacks must not make it one.
    Error {
        #[serde(default)]
        error: WireError,
    },
    #[serde(other)]
    Other,
}

/// The `error` object of an in-band error event: its kind decides the status
/// ([`stands_for`]); its message is shown in the event's own words.
#[derive(Deserialize, Default)]
struct WireError {
    #[serde(rename = "type", default)]
    kind: String,
}

/// The `delta` object of a `message_delta` event. Only `stop_reason` matters
/// to us (`stop_sequence` and `stop_details` are ignored until they earn a
/// consumer); `default` keeps a bare `"delta":{}` parsing.
#[derive(Deserialize, Default)]
struct WireMessageDeltaBody {
    #[serde(default)]
    stop_reason: Option<String>,
}

/// The `message` envelope on `message_start`: its `usage`, and what the API
/// did to the request on the way in.
#[derive(Deserialize)]
struct WireMessageStart {
    #[serde(default)]
    usage: WireUsage,
    /// Present with the binding beta, one entry per block it dropped or let
    /// through unchecked; `[]` when nothing was. Unknown types and reasons
    /// are the API's to add.
    #[serde(default)]
    input_transformations: Vec<WireTransformation>,
}

/// One `input_transformations` entry: `{type, path, reason}`.
#[derive(Deserialize)]
struct WireTransformation {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    reason: String,
}

/// Token counts as they appear on the wire — each field defaults to `0` so a
/// `usage` object carrying only one (the common case) parses cleanly.
///
/// The two cache counts appear on `message_start` when the request carried a
/// `cache_control` breakpoint, and are absent otherwise. They are DISJOINT from
/// `input_tokens`: the API reports what it charged at each rate, so a cached
/// turn's `input_tokens` is only the uncached remainder.
#[derive(Deserialize, Default)]
struct WireUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
}

impl From<WireUsage> for Usage {
    fn from(w: WireUsage) -> Self {
        Usage {
            input_tokens: w.input_tokens,
            output_tokens: w.output_tokens,
            cache_creation_input_tokens: w.cache_creation_input_tokens,
            cache_read_input_tokens: w.cache_read_input_tokens,
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum DeltaKind {
    TextDelta {
        text: String,
    },
    /// A fragment of the open `tool_use` block's input JSON. Fragments are split
    /// at arbitrary points, so only their concatenation is valid JSON.
    InputJsonDelta {
        partial_json: String,
    },
    /// Text of the open thinking block.
    ThinkingDelta {
        thinking: String,
    },
    /// The open thinking block's signature, before it closes.
    SignatureDelta {
        signature: String,
    },
    #[serde(other)]
    Other,
}

/// The `content_block` envelope on `content_block_start`. A text block's
/// content arrives via its deltas, so it folds into `Other` with every type
/// this build does not assemble.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireBlockStart {
    ToolUse {
        id: String,
        name: String,
    },
    /// Usually empty here, its text and signature in the deltas after.
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        signature: String,
    },
    RedactedThinking {
        #[serde(default)]
        data: String,
    },
    #[serde(other)]
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::contract::the_ending;

    /// Parse one standalone event (no tool-call state carried across events).
    fn ev(raw: &str) -> Vec<StreamMsg> {
        parse_sse_event(raw, &mut Assembler::default())
    }

    /// Parse a SEQUENCE of events through one assembler — the way the stream
    /// loop does, so multi-event constructs (a `tool_use` block) come together.
    fn evs(raws: &[&str]) -> Vec<StreamMsg> {
        let mut tools = Assembler::default();
        raws.iter()
            .flat_map(|r| parse_sse_event(r, &mut tools))
            .collect()
    }

    #[test]
    fn parses_text_delta_event() {
        let raw = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n";
        let msg = ev(raw);
        assert_eq!(msg, vec![StreamMsg::Chunk("hello".to_string())]);
    }

    #[test]
    fn parses_message_stop() {
        let raw = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let msg = ev(raw);
        assert_eq!(msg, vec![StreamMsg::Complete]);
    }

    #[test]
    fn ignores_ping_and_unknown_events() {
        let raw = "data: {\"type\":\"ping\"}\n\n";
        assert!(ev(raw).is_empty());
        let raw = "data: {\"type\":\"content_block_start\",\"index\":0}\n\n";
        assert!(ev(raw).is_empty());
    }

    #[test]
    fn parses_usage_from_message_start_and_delta() {
        // `message_start` carries input tokens (output ~0 here).
        let raw = "data: {\"type\":\"message_start\",\"message\":{\"id\":\"x\",\"usage\":{\"input_tokens\":42,\"output_tokens\":1}}}\n\n";
        assert_eq!(
            ev(raw),
            vec![StreamMsg::Usage(Usage {
                input_tokens: 42,
                output_tokens: 1,
                ..Default::default()
            })],
        );
        // `message_delta` with an empty delta carries only the output tokens.
        let raw =
            "data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":99}}\n\n";
        assert_eq!(
            ev(raw),
            vec![StreamMsg::Usage(Usage {
                input_tokens: 0,
                output_tokens: 99,
                ..Default::default()
            })],
        );
        // A `message_start` lacking `usage` still parses (zeros).
        let raw = "data: {\"type\":\"message_start\",\"message\":{\"id\":\"x\"}}\n\n";
        assert_eq!(ev(raw), vec![StreamMsg::Usage(Usage::default())]);
    }

    #[test]
    fn message_delta_carries_the_stop_reason() {
        // The real wire shape: stop_reason rides in `delta`, usage beside it —
        // ONE event, TWO messages. `max_tokens` is the truncation case
        // `:continue` exists for.
        let raw = "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":128}}\n\n";
        assert_eq!(
            ev(raw),
            vec![
                StreamMsg::Usage(Usage {
                    input_tokens: 0,
                    output_tokens: 128,
                    ..Default::default()
                }),
                StreamMsg::Stopped(StopReason::MaxTokens),
            ],
        );
        // An unknown stop reason survives as `Other` — never dropped.
        let raw = "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"novel_reason\"},\"usage\":{\"output_tokens\":1}}\n\n";
        assert_eq!(
            ev(raw)[1],
            StreamMsg::Stopped(StopReason::Other("novel_reason".into())),
        );
    }

    #[test]
    fn ignores_a_delta_it_does_not_know() {
        // Thinking was dropped here until N1; a type this build does not read
        // still is.
        let raw = "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"citations_delta\",\"citation\":{}}}\n\n";
        assert!(ev(raw).is_empty());
    }

    /// **A thinking block streams for the screen and arrives whole** — its
    /// text as it comes, then the block with its signature at its stop, to
    /// send back unchanged; an empty one (`display: "omitted"`) shows
    /// nothing and is kept all the same; an encrypted one is kept as it came.
    /// Between them a call still assembles.
    #[test]
    fn a_thinking_block_streams_and_arrives_whole() {
        let start = |block: &str| {
            format!(
                "data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{block}}}\n\n"
            )
        };
        let delta = |delta: &str| {
            format!("data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{delta}}}\n\n")
        };
        let stop = "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n".to_string();
        let events = [
            start(r#"{"type":"thinking","thinking":"","signature":""}"#),
            delta(r#"{"type":"thinking_delta","thinking":"pla"}"#),
            delta(r#"{"type":"thinking_delta","thinking":"n"}"#),
            delta(r#"{"type":"signature_delta","signature":"SIG"}"#),
            stop.clone(),
            start(r#"{"type":"thinking","thinking":"","signature":""}"#),
            // `omitted` streams empty deltas (Claude Code's capture has one):
            // nothing to show.
            delta(r#"{"type":"thinking_delta","thinking":""}"#),
            delta(r#"{"type":"signature_delta","signature":"S2"}"#),
            stop.clone(),
            start(r#"{"type":"redacted_thinking","data":"R1"}"#),
            stop.clone(),
            start(r#"{"type":"tool_use","id":"t","name":"bash","input":{}}"#),
            delta(r#"{"type":"input_json_delta","partial_json":"{}"}"#),
            stop,
        ];
        let events: Vec<&str> = events.iter().map(String::as_str).collect();
        let thought = |thinking: &str, signature: &str| {
            StreamMsg::Thought(crate::Block::Thinking {
                thinking: thinking.into(),
                signature: signature.into(),
                at: 0,
            })
        };
        assert_eq!(
            evs(&events),
            vec![
                StreamMsg::ThinkingChunk("pla".into()),
                StreamMsg::ThinkingChunk("n".into()),
                thought("plan", "SIG"),
                thought("", "S2"),
                StreamMsg::Thought(crate::Block::RedactedThinking {
                    data: "R1".into(),
                    at: 0,
                }),
                StreamMsg::ToolUse(crate::Block::ToolUse {
                    id: "t".into(),
                    name: "bash".into(),
                    input: serde_json::json!({}),
                }),
            ]
        );
    }

    /// **What the API dropped is said** — each `thinking_dropped` entry on
    /// `message_start`, by its reason; a block let through unchecked
    /// (`thinking_mismatch_allowed`) was not dropped, and `[]` says nothing.
    #[test]
    fn the_thinking_the_api_dropped_is_reported() {
        let raw = "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3},\
                   \"input_transformations\":[\
                   {\"type\":\"thinking_dropped\",\"path\":\"messages.1.content.0\",\"reason\":\"prefix_binding_mismatch\"},\
                   {\"type\":\"thinking_dropped\",\"path\":\"messages.3.content.0\",\"reason\":\"model_binding_mismatch\"},\
                   {\"type\":\"thinking_dropped\",\"path\":\"messages.5.content.0\",\"reason\":\"novel\"},\
                   {\"type\":\"thinking_mismatch_allowed\",\"path\":\"messages.7.content.0\",\"reason\":\"prefix_binding_mismatch\"}]}}\n\n";
        let msgs = ev(raw);
        assert_eq!(
            msgs[1..],
            [StreamMsg::ThinkingDropped(vec![
                crate::Dropped::Prefix,
                crate::Dropped::Model,
                crate::Dropped::Other("novel".into()),
            ])]
        );
        let none = "data: {\"type\":\"message_start\",\"message\":{\"usage\":{},\"input_transformations\":[]}}\n\n";
        assert_eq!(ev(none).len(), 1, "usage alone");
    }

    /// **Thinking is asked for by the model's category** — through the whole
    /// send, the body and its header: adaptive with `block_binding` and the
    /// beta it needs (the field without it is a 400); `updates` adds its own
    /// beta; a budgeted model is sent its budget, and nothing without one;
    /// a model with no generation, nothing.
    #[tokio::test]
    async fn thinking_is_asked_for_by_the_models_category() {
        let reply = || {
            streamed(
                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{}}}\n\n\
                 data: {\"type\":\"message_stop\"}\n\n",
            )
        };
        let heard = |model: &'static str, ask: crate::ThinkingAsk| async move {
            let mut req = SendRequest::new(vec![Message::user("hi")]);
            req.thinking = ask;
            let (head, body, _) = turn_heard(reply(), Default::default(), model, req, |_| {}).await;
            (head, body)
        };
        let ask = |display, budget| crate::ThinkingAsk { display, budget };
        use crate::ThinkingDisplay::{Summarized, Updates};

        let (head, body) = heard("claude-sonnet-5-5", crate::ThinkingAsk::default()).await;
        assert_eq!(
            body["thinking"],
            serde_json::json!({
                "type": "adaptive",
                "display": "summarized",
                "block_binding": {"prefix_mismatch_behavior": "drop_block"},
            })
        );
        assert!(
            head.contains("\r\nanthropic-beta: thinking-binding-controls-2026-08-01"),
            "{head}"
        );
        let (head, body) = heard("claude-opus-4-8", ask(Updates, 0)).await;
        assert_eq!(body["thinking"]["display"], "updates");
        assert!(
            head.contains(
                "anthropic-beta: thinking-binding-controls-2026-08-01,thinking-display-updates-2026-08-18"
            ),
            "{head}"
        );
        let (head, body) = heard("claude-haiku-4-5-20251001", ask(Summarized, 2048)).await;
        assert_eq!(
            body["thinking"],
            serde_json::json!({"type": "enabled", "budget_tokens": 2048})
        );
        assert!(!head.contains("anthropic-beta"), "{head}");
        for model in ["claude-haiku-4-5-20251001", "m"] {
            let (head, body) = heard(model, crate::ThinkingAsk::default()).await;
            assert!(body.get("thinking").is_none(), "{model}: {body}");
            assert!(!head.contains("anthropic-beta"), "{model}: {head}");
        }
    }

    #[test]
    fn malformed_json_does_not_panic() {
        let raw = "data: {not json}\n\n";
        assert!(ev(raw).is_empty());
    }

    /// The real wire shape of a tool call: `content_block_start` names it, the
    /// arguments arrive as `input_json_delta` FRAGMENTS (split at arbitrary
    /// points — each piece is invalid JSON alone), and `content_block_stop`
    /// completes it. The consumer must receive ONE whole request.
    #[test]
    fn assembles_a_tool_use_block_from_fragmented_json() {
        let msgs = evs(&[
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"bash\",\"input\":{}}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"comm\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"and\\\": \\\"ls -l\\\"}\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        ]);
        assert_eq!(
            msgs,
            vec![StreamMsg::ToolUse(crate::Block::ToolUse {
                id: "toolu_1".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "ls -l"}),
            })],
            "one complete request, not fragments"
        );
    }

    /// A tool with no arguments sends no `input_json_delta` at all — that means
    /// `{}`, not a dropped call.
    #[test]
    fn a_tool_use_with_no_input_deltas_is_empty_arguments() {
        let msgs = evs(&[
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t2\",\"name\":\"pwd\",\"input\":{}}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        ]);
        assert_eq!(
            msgs,
            vec![StreamMsg::ToolUse(crate::Block::ToolUse {
                id: "t2".into(),
                name: "pwd".into(),
                input: serde_json::json!({}),
            })]
        );
    }

    /// A TEXT block's start/stop must not manufacture a tool call, and its text
    /// still streams — the assembler stays inert unless a `tool_use` block opened.
    #[test]
    fn a_text_block_does_not_produce_a_tool_use() {
        let msgs = evs(&[
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        ]);
        assert_eq!(msgs, vec![StreamMsg::Chunk("hi".to_string())]);
    }

    /// Unreadable arguments must DROP the call, never guess at one: a tool call
    /// we cannot read is one we must not run. The turn itself survives.
    #[test]
    fn unparseable_tool_input_drops_the_call() {
        let msgs = evs(&[
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t3\",\"name\":\"bash\",\"input\":{}}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\": \"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        ]);
        assert!(msgs.is_empty(), "truncated arguments yield no tool call");
    }

    /// Two tool calls in one turn each come out whole and in order (the model
    /// may batch them; each is gated separately downstream).
    #[test]
    fn two_tool_calls_in_one_turn_stay_separate() {
        let msgs = evs(&[
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"a\",\"name\":\"bash\",\"input\":{}}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\"ls\\\"}\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"b\",\"name\":\"bash\",\"input\":{}}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        ]);
        assert_eq!(
            msgs,
            vec![
                StreamMsg::ToolUse(crate::Block::ToolUse {
                    id: "a".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "ls"}),
                }),
                StreamMsg::ToolUse(crate::Block::ToolUse {
                    id: "b".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "pwd"}),
                }),
            ]
        );
    }

    /// **The system prompt reaches the wire, ahead of the messages.**
    ///
    /// Nothing sent one at all until 2026-08-10, though `request.rs` had
    /// promised the field since M1: the model was handed a `bash` tool and no
    /// context whatsoever — not that it was on somebody's machine, not which
    /// directory, not that looking means running a command.
    #[tokio::test]
    async fn the_system_prompt_is_on_the_wire() {
        let (body, _) = one_turn_with(
            crate::config::CacheConfig::default(),
            "m",
            SendRequest::new(vec![Message::user("hi")]).with_system("you are a teapot"),
        )
        .await;
        assert_eq!(body["system"], "you are a teapot");

        // …and a request without one omits the key entirely, so a turn that
        // takes no system prompt is byte-identical to one from before the
        // field existed.
        let (plain, _) = one_turn_with(
            crate::config::CacheConfig::default(),
            "m",
            SendRequest::new(vec![Message::user("hi")]),
        )
        .await;
        assert!(plain.get("system").is_none(), "{plain}");
    }

    /// **Effort goes out as `output_config.effort`** — through the whole
    /// send path, the bytes on the wire — and a request without one sends no
    /// `output_config` at all: the model's own default, the body unchanged.
    #[tokio::test]
    async fn effort_is_on_the_wire_only_when_asked_for() {
        let (body, _) = one_turn_with(
            crate::config::CacheConfig::default(),
            "m",
            SendRequest::new(vec![Message::user("hi")]).with_effort(Some(crate::Effort::Xhigh)),
        )
        .await;
        assert_eq!(
            body["output_config"],
            serde_json::json!({"effort": "xhigh"})
        );
        let (plain, _) = one_turn_with(
            crate::config::CacheConfig::default(),
            "m",
            SendRequest::new(vec![Message::user("hi")]),
        )
        .await;
        assert!(plain.get("output_config").is_none(), "{plain}");
    }

    /// The shipped prompt says the three things a model cannot infer, and names
    /// the working directory rather than leaving it to guess at paths.
    #[test]
    fn the_shipped_system_prompt_says_where_it_is() {
        let start = std::path::Path::new("/home/me/proj");
        let s = super::super::request::system_prompt(Some(start), None);
        assert!(
            s.contains("/home/me/proj"),
            "the working directory is named: {s}"
        );
        assert!(s.contains("bash"), "the tool is named: {s}");
        assert!(
            s.contains("terminal"),
            "the model is told where its output goes: {s}"
        );
        assert!(
            !s.contains("{cwd}"),
            "the placeholder was substituted, not shipped: {s}"
        );
        assert!(!s.contains("temporary files"), "no scratch, none named");
        // What Shannon does around it: its handles are deliberate, and the
        // manual is the user's `:help` (Steven, 10-20, image 22) — the text
        // Claude Code is given too.
        assert!(s.ends_with(super::super::request::shannon_notes()), "{s}");
        assert!(s.contains("{{secret.1a2b}}") && s.contains(":help"), "{s}");
        // And the scratch directory, when there is one: it is approved, and a
        // model that does not know it reaches for `/tmp`.
        let s = super::super::request::system_prompt(Some(start), Some("/tmp/shannon-501"));
        assert!(s.contains("/tmp/shannon-501"), "{s}");
    }

    /// **A multi-byte reply survives the network, end to end.** Named for the
    /// split it was written to see — which it cannot: reqwest coalesces, so
    /// the bytes written one at a time arrive whole
    /// (`every_possible_chunk_boundary_reassembles`, below, makes the splits
    /// itself, and holds the decode). This holds the path around it (review
    /// stop 12, T9).
    ///
    /// `bytes_stream` yields TCP segments, which respect nothing: an em dash is
    /// three bytes and a segment boundary can fall between any two of them. The
    /// old loop decoded each chunk on arrival, so that returned
    /// `BackendError::Decode` and ended the turn — and the longer the reply, the
    /// more boundaries it had to survive. Every character below is multi-byte
    /// and every one is split at a different offset within its encoding.
    #[tokio::test]
    async fn a_multibyte_reply_survives_end_to_end() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // "…" is 3 bytes, "é" is 2, "🙂" is 4 — split at 1, 1, and 2 bytes in.
        let text = "one… deux é trois 🙂 done";
        let payload = format!(
            "data: {{\"type\":\"content_block_delta\",\"delta\":\
             {{\"type\":\"text_delta\",\"text\":\"{text}\"}}}}\n\n\
             data: {{\"type\":\"message_stop\"}}\n\n"
        );
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                    Connection: close\r\n\r\n";

        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = server.accept().await.expect("the client connects");
            let mut scratch = [0u8; 4096];
            let _ = sock.read(&mut scratch).await;
            let _ = sock.write_all(head.as_bytes()).await;
            // ONE BYTE AT A TIME — the worst case, and it splits every
            // multi-byte character in the payload at every possible offset.
            for byte in payload.as_bytes() {
                let _ = sock.write_all(&[*byte]).await;
                let _ = sock.flush().await;
            }
            let _ = sock.shutdown().await;
        });

        let api = AnthropicApi::new(
            "not-a-real-key".to_string(),
            "m",
            crate::config::CacheConfig::default(),
        )
        .expect("the client builds")
        .with_endpoint(format!("http://{addr}/v1/messages"));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        api.send_message(SendRequest::new(vec![Message::user("hi")]), tx)
            .await
            .expect("the turn task finishes");

        let mut msgs = Vec::new();
        while let Ok(m) = rx.try_recv() {
            msgs.push(m);
        }
        assert!(
            !msgs
                .iter()
                .any(|m| matches!(m, StreamMsg::Error(BackendError::Decode(_)))),
            "a split character is not a malformed response: {msgs:?}"
        );
        assert!(
            msgs.contains(&StreamMsg::Chunk(text.to_string())),
            "the text arrives whole and unmangled: {msgs:?}"
        );
        assert_eq!(the_ending(&msgs), &StreamMsg::Complete);
    }

    /// **Split the same payload at EVERY byte offset; every split reassembles
    /// to the same events.**
    ///
    /// This is the check the end-to-end test cannot make. reqwest coalesces, so
    /// a server writing one byte at a time still arrives in one piece and the
    /// bug is invisible through the socket — that version of this test passed
    /// with the old per-chunk decode still in place. Here the split points are
    /// ours, and the payload is chosen so that most of them fall inside a
    /// character: `…` is 3 bytes, `é` is 2, `🙂` is 4, `世` is 3.
    #[test]
    fn every_possible_chunk_boundary_reassembles() {
        // **Both line endings.** SSE separates events with a blank line, and a
        // line may end `\n` or `\r\n`; only the first was recognised, and the
        // fixture used only the first, so it could not have said so — a fixture
        // that omits the axis cannot test the axis
        // (`[[feedback_fixture_omits_the_axis]]`). A proxy that normalises line
        // endings, which is what such a thing is *for*, made the stream
        // unreadable: no separator found, the buffer fills to the cap, and the
        // error blames the event's size.
        for eol in ["\n", "\r\n"] {
            let payload = format!(
                "data: {{\"type\":\"content_block_delta\",\"delta\":\
                 {{\"type\":\"text_delta\",\"text\":\"one… é 🙂 世界\"}}}}{eol}{eol}\
                 data: {{\"type\":\"message_stop\"}}{eol}{eol}"
            );
            let bytes = payload.as_bytes();

            // The whole thing in one piece — the reference the splits must match.
            let mut whole = SseBuffer::default();
            whole.push(bytes).unwrap();
            let mut expected = Vec::new();
            while let Some(e) = whole.next_event() {
                expected.push(e.expect("valid UTF-8"));
            }
            assert_eq!(expected.len(), 2, "premise: two events with {eol:?}");
            // …and they really parse, or "reassembles" would be a claim about
            // byte runs rather than about events.
            let mut tools = Assembler::default();
            assert!(
                !parse_sse_event(&expected[0], &mut tools).is_empty(),
                "premise: the reassembled event is one the parser understands"
            );

            for split in 0..=bytes.len() {
                let mut buf = SseBuffer::default();
                buf.push(&bytes[..split]).unwrap();
                let mut got = Vec::new();
                while let Some(e) = buf.next_event() {
                    got.push(e.unwrap_or_else(|err| {
                        panic!("split at {split} produced a decode error: {err}")
                    }));
                }
                buf.push(&bytes[split..]).unwrap();
                while let Some(e) = buf.next_event() {
                    got.push(e.unwrap_or_else(|err| {
                        panic!("split at {split} produced a decode error: {err}")
                    }));
                }
                assert_eq!(got, expected, "{eol:?}, split at byte {split}");
            }
        }
    }

    /// An event still arriving yields nothing; a whole one that genuinely is
    /// not UTF-8 is a real protocol error and says so rather than vanishing.
    #[test]
    fn a_partial_event_waits_and_a_broken_one_is_reported() {
        let mut buf = SseBuffer::default();
        buf.push(b"data: x\n").unwrap();
        assert!(buf.next_event().is_none(), "still arriving");
        buf.push(b"\n").unwrap();
        assert_eq!(buf.next_event().unwrap().unwrap(), "data: x\n\n");

        let mut buf = SseBuffer::default();
        buf.push(&[b'd', 0xff, 0xfe, b'\n', b'\n']).unwrap();
        assert!(
            matches!(buf.next_event(), Some(Err(BackendError::Decode(_)))),
            "a whole event that is not UTF-8 is reported, not dropped"
        );
    }

    /// The overflow guard still fires — across many small reads, and on one
    /// oversized read, which used to allocate in full and only then be
    /// rejected (Reviewer B).
    #[test]
    fn a_stream_with_no_event_boundary_is_capped() {
        let mut buf = SseBuffer::default();
        let chunk = vec![b'x'; 64 * 1024];
        let mut err = None;
        for _ in 0..64 {
            if let Err(e) = buf.push(&chunk) {
                err = Some(e);
                break;
            }
        }
        assert!(
            matches!(err, Some(BackendError::Decode(_))),
            "the cap must trip before memory runs out"
        );

        // One read larger than the cap is refused WITHOUT copying it in.
        let mut buf = SseBuffer::default();
        let huge = vec![b'x'; MAX_SSE_BUFFER + 1];
        assert!(matches!(buf.push(&huge), Err(BackendError::Decode(_))));
        assert_eq!(buf.bytes.len(), 0, "the oversized read was never copied in");
    }

    /// **The separator scan does not restart at byte zero on every read.**
    ///
    /// It did, and there is one read per TCP segment, so a single large event
    /// arriving in small pieces was O(n²): Reviewer B pegged a core for over
    /// 160 seconds without reaching the 1 MiB cap. `run_turn` is a spawned task
    /// with no `.await` in that loop, so it is a pinned worker thread — the
    /// same failure this session fixed on the render side, reopened on the
    /// parse side by the fix for the character-split bug.
    ///
    /// **Asserted as WORK DONE, not as elapsed time.** The first version of
    /// this filled 200k bytes and relied on the quadratic version being too
    /// slow to finish — and in release it finished in 70 seconds and PASSED.
    /// A guessed margin is not a measurement, which is the shape this project
    /// keeps paying for. Counting comparisons removes the margin entirely:
    /// quadratic fails at any size, so the test stays small and instant.
    #[test]
    fn the_separator_scan_does_not_rescan_from_the_start() {
        const N: usize = 20_000;
        let mut buf = SseBuffer::default();
        for _ in 0..N {
            buf.push(b"x").expect("under the cap");
            assert!(buf.next_event().is_none(), "no boundary yet");
        }
        // Linear is ~N comparisons in total; restarting at zero every push is
        // ~N²/2, which is 10,000× more. Three times N leaves room for the
        // one-byte overlap each scan re-examines and nothing else.
        assert!(
            buf.compared <= 3 * N,
            "the scan is not linear: {} comparisons over {N} bytes",
            buf.compared
        );

        buf.push(b"\n\n").expect("under the cap");
        let event = buf.next_event().expect("the boundary is found").unwrap();
        assert_eq!(event.len(), N + 2);
        // …and the cursor reset, so the next event is found from the start.
        buf.push(b"a\n\n").expect("room");
        assert_eq!(buf.next_event().unwrap().unwrap(), "a\n\n");
    }

    /// **A tool call cut off mid-stream is reported, not dropped.**
    ///
    /// THE demo bug. The model asked to run a command, the reply hit its length
    /// ceiling part-way through the arguments, `content_block_stop` never
    /// arrived, and `run_turn` dropped the assembler on the floor: no error, no
    /// warning, no transcript entry. Four turns billed 4,096 output tokens and
    /// rendered nothing at all. Whatever else is true of a failure, it has to
    /// reach a human.
    #[tokio::test]
    async fn a_tool_call_cut_off_mid_stream_is_reported() {
        // A `tool_use` block opens, its arguments start arriving, and the
        // stream simply ends — exactly what the token ceiling produces.
        let msgs = answered(streamed(
            "data: {\"type\":\"content_block_start\",\"content_block\":\
             {\"type\":\"tool_use\",\"id\":\"tu_1\",\"name\":\"bash\"}}\n\n\
             data: {\"type\":\"content_block_delta\",\"delta\":\
             {\"type\":\"input_json_delta\",\"partial_json\":\
             \"{\\\"command\\\": \\\"echo one two thr\"}}\n\n",
        ))
        .await;
        // An abandoned tool call reaches the consumer, as the turn's ending.
        let StreamMsg::Error(err @ BackendError::Truncated { tool, .. }) = the_ending(&msgs) else {
            panic!("an abandoned tool call must reach the consumer: {msgs:?}");
        };
        assert_eq!(tool, "bash");
        // And it must name the recovery, since the user cannot infer it.
        // DISPLAY, not Debug: the TUI shows `Display`, and asserting the Debug
        // form would pass on a variant whose user-facing message said nothing.
        let shown = err.to_string();
        assert!(shown.contains("max_output_tokens"), "{shown}");
        assert!(shown.contains("bash"), "{shown}");
    }

    /// **A stream that stops with no `message_stop` ends as a broken one** —
    /// said, once, and last: the consumer's turn gate waits on an ending, and
    /// without this one it waited for good. Held by nothing: making it `Ok`
    /// passed both crates (review stop 12, T9 4).
    #[tokio::test]
    async fn a_stream_cut_short_ends_once_as_a_broken_one() {
        let msgs = answered(streamed(
            "data: {\"type\":\"content_block_delta\",\"delta\":\
             {\"type\":\"text_delta\",\"text\":\"par\"}}\n\n",
        ))
        .await;
        assert!(msgs.contains(&StreamMsg::Chunk("par".into())), "{msgs:?}");
        assert_eq!(
            the_ending(&msgs),
            &StreamMsg::Error(BackendError::Transport(
                "stream ended before message_stop".into()
            ))
        );
    }

    /// **An `error` event mid-stream ends the turn as that error** — the API's
    /// streaming form of an HTTP failure, after a 200. It had no arm: it fell
    /// to `Other`, and an overloaded API was reported as "stream ended before
    /// message_stop", a broken connection (review stop 12, T9 5). What arrived
    /// before it stays, and ONE ending follows — not the error and then the
    /// "ended before" that a missing `message_stop` also earns.
    #[tokio::test]
    async fn an_error_event_mid_stream_ends_the_turn_as_that_error() {
        let error =
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let msgs = answered(streamed(&format!(
            "data: {{\"type\":\"content_block_delta\",\"delta\":\
             {{\"type\":\"text_delta\",\"text\":\"par\"}}}}\n\n\
             data: {error}\n\n"
        )))
        .await;
        assert!(msgs.contains(&StreamMsg::Chunk("par".into())), "{msgs:?}");
        assert_eq!(
            the_ending(&msgs),
            &StreamMsg::Error(BackendError::Api {
                status: 529,
                message: error.into(),
            })
        );
    }

    /// **One failure reads the same however the API reports it** — as a
    /// status with a body, or in-band standing for that status: 401 is the
    /// key, 429 is waiting, any other its status and the server's words. Every
    /// kind the reference lists, one it does not, and an event with nothing
    /// but its tag. The status path's two were held by nothing (T9 5: 401 no
    /// longer `Auth`, GREEN).
    #[tokio::test]
    async fn a_failure_reads_the_same_as_a_status_or_in_band() {
        let failure = |status: u16, body: &str| match status {
            401 => BackendError::Auth,
            429 => BackendError::RateLimited,
            status => BackendError::Api {
                status,
                message: body.into(),
            },
        };
        let listed = [
            ("invalid_request_error", 400),
            ("authentication_error", 401),
            ("permission_error", 403),
            ("not_found_error", 404),
            ("request_too_large", 413),
            ("rate_limit_error", 429),
            ("api_error", 500),
            ("overloaded_error", 529),
        ];
        for (kind, status) in listed {
            let body = format!(r#"{{"type":"error","error":{{"type":"{kind}","message":"m"}}}}"#);
            let expected = StreamMsg::Error(failure(status, &body));
            assert_eq!(
                ev(&format!("data: {body}")),
                std::slice::from_ref(&expected),
                "in-band {kind}"
            );
            let msgs = answered(refused(&format!("{status} X"), &body)).await;
            assert_eq!(the_ending(&msgs), &expected, "status {status}");
        }
        for unlisted in [
            r#"{"type":"error","error":{"type":"a_kind_not_yet_listed"}}"#,
            r#"{"type":"error","error":{"message":"no kind"}}"#,
            r#"{"type":"error"}"#,
        ] {
            let expected = StreamMsg::Error(failure(500, unlisted));
            assert_eq!(ev(&format!("data: {unlisted}")), [expected], "{unlisted}");
        }
    }

    /// **What the server says arrives without control characters, by either
    /// path** — it goes to the log file, which does not sanitize, and to the
    /// screen. A clear-screen and a newline (a forged log line): neither
    /// survives, the words do. The status path's filter was held by nothing
    /// (T9 5: deleting it, GREEN).
    #[tokio::test]
    async fn what_the_server_says_arrives_without_control_characters() {
        let in_band = ev(
            r#"data: {"type":"error","error":{"type":"api_error","message":"\u001b[2Jgone\nERROR forged"}}"#,
        );
        let by_status = answered(refused("500 X", "\u{1b}[2Jgone\nERROR forged")).await;
        for (path, msgs) in [("in-band", in_band), ("status", by_status)] {
            let StreamMsg::Error(BackendError::Api {
                status: 500,
                message,
            }) = the_ending(&msgs)
            else {
                panic!("{path}: {msgs:?}");
            };
            assert!(
                !message.chars().any(char::is_control),
                "{path}: {message:?}"
            );
            assert!(
                message.contains("gone") && message.contains("ERROR forged"),
                "{path}: the words survive: {message:?}"
            );
        }
    }

    /// The reply ceiling comes from the model the request actually names, not
    /// from the backend's own default — a `:model opus` session must not
    /// inherit the fallback belonging to the model the backend was built with.
    ///
    /// **Asserted on the wire, not on the table.** The first version of this
    /// compared `max_output_tokens("claude-opus-5")` with the unknown-model
    /// fallback and called that a test: it measured the premise, survived a
    /// mutation that made `send_message` ignore the request's model entirely,
    /// and would have shipped a `:model opus` session capped at 8,192
    /// ([[feedback_assert_the_subject]]).
    #[tokio::test]
    async fn the_reply_ceiling_follows_the_requests_model() {
        let instance = "a-model-we-do-not-know";
        let asked = "claude-opus-5";
        assert_ne!(
            super::super::models::max_output_tokens(instance),
            super::super::models::max_output_tokens(asked),
            "premise: the two ceilings differ, so the wiring is observable"
        );

        let (body, _) = one_turn_with(
            crate::config::CacheConfig::default(),
            instance,
            SendRequest::new(vec![Message::user("hi")]).with_model(asked),
        )
        .await;
        assert_eq!(body["model"], asked);
        assert_eq!(
            body["max_tokens"],
            super::super::models::max_output_tokens(asked),
            "the ceiling followed the backend's model, not the request's: {body}"
        );
    }

    /// **`message_start` carries the cache counts, and we keep them.**
    ///
    /// This is where the billing readout comes from: with a breakpoint on the
    /// request, the API reports what it charged at each rate, and `input_tokens`
    /// shrinks to whatever fell after the breakpoint. Dropping the two cache
    /// fields — which is what the parser did before — left a long cached
    /// conversation reporting a two-digit prompt.
    #[test]
    fn message_start_keeps_the_cache_counts() {
        let msgs = ev(concat!(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":12,"#,
            r#""cache_creation_input_tokens":134,"cache_read_input_tokens":1100}}}"#,
            "\n\n"
        ));
        assert_eq!(
            msgs,
            vec![StreamMsg::Usage(Usage {
                input_tokens: 12,
                output_tokens: 0,
                cache_creation_input_tokens: 134,
                cache_read_input_tokens: 1100,
            })],
        );
        // An uncached turn omits both keys entirely; they must default to 0
        // rather than failing the whole event.
        let plain = ev(
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":42}}}\n\n",
        );
        assert_eq!(
            plain,
            vec![StreamMsg::Usage(Usage {
                input_tokens: 42,
                ..Usage::default()
            })],
        );
    }

    /// **The breakpoint is on every request, or the trail has holes in it.**
    ///
    /// The rolling entry at each request's tail is what a later branch reads;
    /// one request that forgot to mark its tail is one turn boundary nobody can
    /// branch from cheaply. So the policy is the backend's, and this asserts the
    /// shape it puts on the wire — including that `5m` is the *absence* of a
    /// `ttl`, which is how the API spells its default.
    #[test]
    fn the_cache_breakpoint_rides_on_the_request() {
        use crate::config::{CacheConfig, CacheTtl};

        let five = CacheControl::from_config(CacheConfig::default());
        assert_eq!(
            serde_json::to_value(five).unwrap(),
            serde_json::json!({"type": "ephemeral"}),
            "the five-minute default sends no ttl at all",
        );

        let hour = CacheControl::from_config(CacheConfig {
            enabled: true,
            ttl: CacheTtl::OneHour,
        });
        assert_eq!(
            serde_json::to_value(hour).unwrap(),
            serde_json::json!({"type": "ephemeral", "ttl": "1h"}),
        );

        let off = CacheControl::from_config(CacheConfig {
            enabled: false,
            ttl: CacheTtl::OneHour,
        });
        assert_eq!(off, None, "disabled wins over any ttl");

        // …and off means the key is absent from the body, not present-and-null:
        // a request is then byte-identical to one from before caching existed.
        let msgs = vec![Message::user("hi")];
        let body = serde_json::to_value(RequestBody {
            model: "m",
            max_tokens: 8,
            stream: true,
            cache_control: off,
            system: None,
            messages: &msgs,
            tools: &[],
            output_config: None,
            thinking: None,
        })
        .unwrap();
        assert!(
            body.get("cache_control").is_none(),
            "caching off must leave no trace: {body}"
        );
        let on = serde_json::to_value(RequestBody {
            model: "m",
            max_tokens: 8,
            stream: true,
            cache_control: five,
            system: None,
            messages: &msgs,
            tools: &[],
            output_config: None,
            thinking: None,
        })
        .unwrap();
        assert_eq!(
            on["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
    }

    /// A turn offering no tools serialises no `tools` key at all — an existing
    /// plain conversation's request body is byte-for-byte unchanged.
    #[test]
    fn no_tools_means_no_tools_key_on_the_wire() {
        let msgs = vec![Message::user("hi")];
        let plain = RequestBody {
            model: "m",
            max_tokens: 8,
            stream: true,
            cache_control: None,
            system: None,
            messages: &msgs,
            tools: &[],
            output_config: None,
            thinking: None,
        };
        assert_eq!(
            serde_json::to_value(&plain).unwrap(),
            serde_json::json!({
                "model": "m",
                "max_tokens": 8,
                "stream": true,
                "messages": [{"role": "user", "content": "hi"}],
            }),
        );

        // With a tool, it rides along in the Anthropic `tools` shape.
        let tools = vec![ToolDef::bash()];
        let with = RequestBody {
            model: "m",
            max_tokens: 8,
            stream: true,
            cache_control: None,
            system: None,
            messages: &msgs,
            tools: &tools,
            output_config: None,
            thinking: None,
        };
        let body = serde_json::to_value(&with).unwrap();
        assert_eq!(body["tools"][0]["name"], "bash");
        assert_eq!(body["tools"][0]["input_schema"]["required"][0], "command");
    }
    /// Run one whole turn against a local socket and hand back **the bytes we
    /// actually sent** plus everything the stream produced.
    ///
    /// The point is the four hops between a [`CacheConfig`] and a request body:
    /// `send_message` reading `self.cache`, [`CacheControl::from_config`],
    /// [`Outgoing`], and [`RequestBody`]. Any one of them could drop the
    /// breakpoint and leave a suite that only serialises structs entirely
    /// green — the caching would just quietly never happen, at full price.
    async fn one_turn(cache: crate::config::CacheConfig) -> (serde_json::Value, Vec<StreamMsg>) {
        one_turn_with(cache, "m", SendRequest::new(vec![Message::user("hi")])).await
    }

    /// [`one_turn`], with the backend's own model and the request spelled out —
    /// the two things a per-model default has to tell apart.
    async fn one_turn_with(
        cache: crate::config::CacheConfig,
        instance_model: &str,
        req: SendRequest,
    ) -> (serde_json::Value, Vec<StreamMsg>) {
        one_turn_then(cache, instance_model, req, |_| {}).await
    }

    /// [`one_turn_with`], doing `adjust` to the backend between building it
    /// and sending — what happens to a live backend mid-session.
    async fn one_turn_then(
        cache: crate::config::CacheConfig,
        instance_model: &str,
        req: SendRequest,
        adjust: impl FnOnce(&AnthropicApi),
    ) -> (serde_json::Value, Vec<StreamMsg>) {
        // A minimal but REAL stream: usage carrying cache counts, one text
        // delta, and the `message_stop` the turn gate waits for.
        let reply = streamed(
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":\
             {\"input_tokens\":12,\"cache_read_input_tokens\":1100,\
             \"cache_creation_input_tokens\":134}}}\n\n\
             data: {\"type\":\"content_block_delta\",\"delta\":\
             {\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n\
             data: {\"type\":\"message_stop\"}\n\n",
        );
        turn_answered(reply, cache, instance_model, req, adjust).await
    }

    /// What one plain turn sends its consumer when the API answers `reply`.
    async fn answered(reply: String) -> Vec<StreamMsg> {
        let req = SendRequest::new(vec![Message::user("hi")]);
        turn_answered(reply, Default::default(), "m", req, |_| {})
            .await
            .1
    }

    /// A 200 carrying `events`. No Content-Length — the close is the end of
    /// the body.
    fn streamed(events: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
             Connection: close\r\n\r\n{events}"
        )
    }

    /// A non-2xx `status` (`"529 Overloaded"`) whose body is `body`.
    fn refused(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// **One turn against a server that answers `reply` verbatim** — any
    /// status line, headers and body — returning the request's body as sent
    /// and every message the turn gave its consumer.
    async fn turn_answered(
        reply: String,
        cache: crate::config::CacheConfig,
        instance_model: &str,
        req: SendRequest,
        adjust: impl FnOnce(&AnthropicApi),
    ) -> (serde_json::Value, Vec<StreamMsg>) {
        let (_, body, msgs) = turn_heard(reply, cache, instance_model, req, adjust).await;
        (body, msgs)
    }

    /// [`turn_answered`], with the request's head as the server read it —
    /// its headers lowercased, for a header a field needs beside it.
    async fn turn_heard(
        reply: String,
        cache: crate::config::CacheConfig,
        instance_model: &str,
        req: SendRequest,
        adjust: impl FnOnce(&AnthropicApi),
    ) -> (String, serde_json::Value, Vec<StreamMsg>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        let (body_tx, body_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut sock, _) = server.accept().await.expect("the client connects");
            // Read until the whole body has arrived: headers end at the blank
            // line, and `Content-Length` says how much follows.
            let mut raw = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = sock.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&raw).to_string();
                let Some(head_end) = text.find("\r\n\r\n") else {
                    continue;
                };
                let len: usize = text[..head_end]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")?
                            .trim()
                            .parse()
                            .ok()
                    })
                    .unwrap_or(0);
                if raw.len() >= head_end + 4 + len {
                    let body = raw[head_end + 4..head_end + 4 + len].to_vec();
                    let _ = body_tx.send((text[..head_end].to_ascii_lowercase(), body));
                    break;
                }
            }
            let _ = sock.write_all(reply.as_bytes()).await;
            let _ = sock.flush().await;
            let _ = sock.shutdown().await;
        });

        let api = AnthropicApi::new("not-a-real-key".to_string(), instance_model, cache)
            .expect("the client builds")
            .with_endpoint(format!("http://{addr}/v1/messages"));
        adjust(&api);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        api.send_message(req, tx)
            .await
            .expect("the turn task finishes");

        let mut msgs = Vec::new();
        while let Ok(m) = rx.try_recv() {
            msgs.push(m);
        }
        let (head, body) = body_rx.await.expect("the server read a body");
        (
            head,
            serde_json::from_slice(&body).expect("the body is JSON"),
            msgs,
        )
    }

    /// **A context block leaves as text, ahead of the prompt** — through the
    /// whole send, not a serialised struct: the store's `context` type is
    /// not the API's, and a request carrying it would be refused.
    #[tokio::test]
    async fn a_context_block_is_text_on_the_wire() {
        let mut user = Message::user("go");
        user.blocks.push(crate::Block::Context {
            kind: crate::ContextKind::Instructions,
            source: "/p/AGENTS.md".into(),
            text: "be terse".into(),
        });
        let req = SendRequest::new(vec![user]);
        let (body, _) = one_turn_with(Default::default(), "m", req).await;
        let content = &body["messages"][0]["content"];
        assert_eq!(content[0]["type"], "text", "{body}");
        let first = content[0]["text"].as_str().unwrap_or_default();
        assert!(
            first.contains("be terse") && first.contains("/p/AGENTS.md"),
            "{first}"
        );
        assert_eq!(
            content[1],
            serde_json::json!({"type": "text", "text": "go"})
        );
        assert!(!body.to_string().contains("\"context\""), "{body}");
    }

    /// **The breakpoint reaches the socket**, and the cache counts come back.
    #[tokio::test]
    async fn a_real_request_carries_the_cache_breakpoint() {
        let (body, msgs) = one_turn(crate::config::CacheConfig::default()).await;
        assert_eq!(
            body["cache_control"],
            serde_json::json!({"type": "ephemeral"}),
            "the config policy reached the wire: {body}"
        );
        assert!(
            msgs.contains(&StreamMsg::Usage(Usage {
                input_tokens: 12,
                output_tokens: 0,
                cache_creation_input_tokens: 134,
                cache_read_input_tokens: 1100,
            })),
            "the cache counts survive the round trip: {msgs:?}"
        );
        assert_eq!(the_ending(&msgs), &StreamMsg::Complete);
    }

    /// …and turning it off leaves no trace on the wire, so a user who says no
    /// sends exactly what this program sent before caching existed.
    #[tokio::test]
    async fn caching_off_sends_no_cache_control() {
        let (body, _) = one_turn(crate::config::CacheConfig {
            enabled: false,
            ttl: crate::config::CacheTtl::FiveMinutes,
        })
        .await;
        assert!(body.get("cache_control").is_none(), "{body}");
    }

    /// **A cache policy changed mid-session reaches the NEXT request** — the
    /// backend is still the one place that decides, and `set_cache` is how it
    /// changes its mind (`:set nocache.enabled` echoed success while every
    /// request kept the launch policy: Reviewers A and C, 2026-09-29).
    #[tokio::test]
    async fn a_changed_cache_policy_reaches_the_next_request() {
        use crate::config::{CacheConfig, CacheTtl};
        let off = CacheConfig {
            enabled: false,
            ttl: CacheTtl::FiveMinutes,
        };
        let (body, _) = one_turn_then(
            CacheConfig::default(),
            "m",
            SendRequest::new(vec![Message::user("hi")]),
            |api| api.set_cache(off),
        )
        .await;
        assert!(body.get("cache_control").is_none(), "turned off: {body}");

        let hour = CacheConfig {
            enabled: true,
            ttl: CacheTtl::OneHour,
        };
        let (body, _) = one_turn_then(
            off,
            "m",
            SendRequest::new(vec![Message::user("hi")]),
            |api| api.set_cache(hour),
        )
        .await;
        assert_eq!(
            body["cache_control"],
            serde_json::json!({"type": "ephemeral", "ttl": "1h"}),
            "turned on, with the new ttl: {body}"
        );
    }

    /// **The client refuses to follow a redirect**, which is the only thing
    /// stopping `x-api-key` being replayed to a host we did not choose.
    ///
    /// `x-api-key` is a *custom* header, and reqwest forwards custom headers
    /// across a cross-host redirect — it strips only the well-known
    /// `Authorization` and `Cookie`. So a permissive policy would hand the key
    /// to whatever answers a 302. The four-line comment on `http_client`
    /// explaining that was, until this test, the entire control: mutating
    /// `Policy::none()` to `Policy::limited(10)` left the whole suite green
    /// (Reviewer E).
    ///
    /// The shape is deliberately deterministic rather than timed: the redirect
    /// points at a port nothing is listening on, so a **following** client
    /// fails to connect and returns `Err`, while a **refusing** client hands
    /// back the 302 itself. No sleeps, no margin to guess wrong about.
    #[tokio::test]
    async fn the_client_does_not_follow_redirects() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // A port we bind and immediately drop: nothing will answer there, so
        // following the redirect is observable as a connection failure.
        let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_port = dead.local_addr().unwrap().port();
        drop(dead);

        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = server.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let body = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{dead_port}/\r\n\
                     Content-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = sock.write_all(body.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });

        let response = http_client()
            .expect("the client builds")
            .get(format!("http://{addr}/v1/messages"))
            .header("x-api-key", "not-a-real-key")
            .send()
            .await;

        let status = response
            .expect(
                "the request did not follow the redirect, so it never tried the \
                 dead port — a following client fails to connect here",
            )
            .status();
        assert_eq!(
            status.as_u16(),
            302,
            "the 302 is handed back rather than chased, so the key never \
             reaches a host we did not choose"
        );
    }

    /// **A reply that keeps arriving is read to the end; one that goes quiet
    /// is not** — the deadline is on silence, not on length ([`STALL_TIMEOUT`]).
    /// At a scale a test can wait for: a 300 ms stall, a body that trickles
    /// for a second in 100 ms steps (any TOTAL deadline under a second kills
    /// it), and one that pauses 600 ms.
    #[tokio::test]
    async fn a_long_reply_is_read_and_a_stalled_one_is_not() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        /// A server that answers 200, then writes one byte after each gap.
        async fn trickling(gaps: &'static [u64]) -> String {
            let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = server.local_addr().unwrap();
            tokio::spawn(async move {
                let Ok((mut sock, _)) = server.accept().await else {
                    return;
                };
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let head = "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";
                let _ = sock.write_all(head.as_bytes()).await;
                for gap in gaps {
                    tokio::time::sleep(Duration::from_millis(*gap)).await;
                    if sock.write_all(b"x").await.is_err() {
                        return;
                    }
                    let _ = sock.flush().await;
                }
                let _ = sock.shutdown().await;
            });
            format!("http://{addr}/")
        }

        let client = client_stalling_after(Duration::from_millis(300)).unwrap();
        let read = |url: String| {
            let client = client.clone();
            async move { client.get(url).send().await?.text().await }
        };

        let long = read(trickling(&[100; 10]).await).await;
        assert_eq!(
            long.expect("still arriving after a second, and never quiet for 300 ms"),
            "x".repeat(10)
        );
        let stalled = read(trickling(&[50, 600, 50]).await).await;
        let err = stalled.expect_err("600 ms of silence is a stall");
        // And it is SAID as one: reqwest's own text names only the kind.
        assert!(
            err.is_timeout(),
            "premise: reqwest calls it a timeout: {err:?}"
        );
        let said = transport(err).to_string();
        assert!(said.contains("no data from the API for 300 s"), "{said}");
    }
}
