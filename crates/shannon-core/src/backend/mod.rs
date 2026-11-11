//! Backend trait + first implementations.
//!
//! A `Backend` turns a conversation history into a stream of
//! `StreamMsg`. M1 ships two: `CannedStub` (offline) and `AnthropicApi`
//! (real Claude via the public Messages API). OAuth-based subscription
//! auth is a planned third; see TODO.md.
//!
//! Implementations spawn a tokio task and return its `JoinHandle` so
//! the caller can `abort()` mid-stream — the TUI uses this to cancel
//! an in-flight turn cleanly when the user wants to switch
//! conversations or quit.

pub mod anthropic;
pub mod claude_code;
pub mod models;
pub mod request;
pub mod stub;

use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::StreamMsg;

/// Runs one turn — sends `req` to some sink and emits the assistant's
/// reply chunk-by-chunk onto `tx`. Returns the spawned task's handle
/// so the caller can abort it.
///
/// **Its stream ends in exactly one `Complete` or `Error`, and nothing
/// after it** — the message the consumer's turn gate waits for: with none
/// the turn waits for good, and a second lands on a turn already closed.
/// Each backend holds this alone (the TUI's forwarder only ends when the
/// sender drops); `contract::the_ending` checks it in every backend's
/// tests. The recording doubles send nothing at all: their tests are the
/// stream.
///
/// Object-safe (`&self`, no generic methods) so the TUI can hold a
/// `Box<dyn Backend>` / `Arc<dyn Backend>` and swap stub-for-real (or
/// one model for another) without changing the event loop. The input
/// is a [`SendRequest`] struct rather than positional args so per-turn
/// config can grow without breaking the trait.
pub trait Backend: Send + Sync {
    fn send_message(&self, req: SendRequest, tx: UnboundedSender<StreamMsg>) -> JoinHandle<()>;

    /// The model this backend talks to by default (what a turn with
    /// `SendRequest.model == None` actually runs against). Lets the caller
    /// record the *resolved* model in a turn's provenance instead of a
    /// placeholder — so the viewer/inspector shows what really produced a turn.
    fn model(&self) -> &str;

    /// **Take a changed cache policy** (`[cache]` changed by `:set` or a
    /// `:reload`) for every request from the next one on. The backend stays
    /// the one place that decides what a request's `cache_control` is (see
    /// `AnthropicApi::new`); this only lets it change its mind. A backend with
    /// nothing to cache ignores it.
    fn set_cache(&self, _cache: crate::config::CacheConfig) {}

    /// **Answer a [`StreamMsg::ToolAsk`]** — for a provider that runs its own
    /// tools, the gate's verdict on one of them, sent to exactly the question
    /// `ask` names ([`AskId`](crate::AskId)'s serial, never the call's id).
    /// `false` when there is no such question waiting (it was answered, or
    /// its turn ended — and an unanswered question is refused then), or this
    /// backend asks none.
    fn answer_tool(&self, _ask: &crate::AskId, _reply: crate::hook::Reply) -> bool {
        false
    }
}

pub use anthropic::AnthropicApi;
pub use models::resolve as resolve_model;
pub use request::{ProviderEnv, SendRequest, ThinkingAsk, ToolDef};
pub use stub::CannedStub;
/// Test doubles, behind `test-util` — see the feature comment in `Cargo.toml`.
#[cfg(any(test, feature = "test-util"))]
pub use stub::{HangingBackend, RecordingProvider, RecordingStub};

/// What every backend's stream is held to ([`Backend::send_message`]).
#[cfg(test)]
pub(crate) mod contract {
    use crate::StreamMsg;

    /// **The one ending of a turn's stream**, for the caller to judge —
    /// asserting there is exactly one `Complete` or `Error`, and it last.
    pub(crate) fn the_ending(msgs: &[StreamMsg]) -> &StreamMsg {
        let ends: Vec<usize> = (msgs.iter().enumerate())
            .filter(|(_, m)| matches!(m, StreamMsg::Complete | StreamMsg::Error(_)))
            .map(|(at, _)| at)
            .collect();
        assert_eq!(ends.len(), 1, "one ending: {msgs:?}");
        assert_eq!(ends[0] + 1, msgs.len(), "the ending is last: {msgs:?}");
        &msgs[ends[0]]
    }
}
