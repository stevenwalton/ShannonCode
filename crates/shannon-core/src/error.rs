//! Backend failure categories.
//!
//! A `thiserror`-derived enum rather than a string so callers can
//! *match on the failure mode* rather than parse prose — the
//! distinction matters the moment a consumer wants to react
//! differently (retry on rate-limit, re-auth on `Auth`, surface a
//! transport blip differently from an API rejection). M1's TUI only
//! renders the `Display` form inline, but the structure is in place
//! for M2's OAuth backend, which needs exactly that branching.
//!
//! The variants deliberately carry only `Clone + PartialEq + Eq`
//! data (no `reqwest::Error`, which is none of those) so the whole
//! type can ride inside `StreamMsg`, which the TUI compares in
//! tests. Transport / decode failures are stringified at the
//! boundary where they occur.

/// A categorized backend failure. Carried by `StreamMsg::Error`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    /// Authentication rejected (HTTP 401). The key is wrong, expired,
    /// or revoked. For an OAuth backend this is the re-auth trigger.
    #[error("authentication failed (HTTP 401) — check your API key")]
    Auth,

    /// Rate limited (HTTP 429). A retry with backoff is the natural
    /// reaction; M1 just surfaces it.
    #[error("rate limited (HTTP 429) — slow down and retry")]
    RateLimited,

    /// Any other non-success HTTP status, with a truncated body for
    /// diagnosis.
    #[error("API error (HTTP {status}): {message}")]
    Api { status: u16, message: String },

    /// The request never completed: connection refused, TLS failure,
    /// timeout, a broken stream mid-response. Stringified from the
    /// underlying `reqwest::Error` at the call site.
    #[error("network error: {0}")]
    Transport(String),

    /// The response arrived but could not be decoded (non-UTF-8
    /// bytes, or — in principle — a shape we cannot parse at all).
    #[error("malformed response: {0}")]
    Decode(String),

    /// The reply was cut off part-way through a tool call, so the call cannot
    /// be run — the model asked for something and we cannot tell what.
    ///
    /// Its own variant rather than a `Decode` string because the recovery is
    /// specific and worth naming: the answer hit its length ceiling, so raising
    /// `[turn] max_output_tokens` (or `:continue`) is the fix, and the message
    /// says so. This spent four turns of a live demo producing nothing at all,
    /// silently, because the half-written block was simply dropped.
    #[error(
        "the reply was cut off while calling `{tool}` ({bytes} bytes in), so the \
         command could not be run — raise `[turn] max_output_tokens`"
    )]
    Truncated { tool: String, bytes: usize },

    /// **Claude Code (`claude -p`) could not run the turn**, or reported it
    /// failed — not installed, not logged in, exited mid-turn, or no gate to
    /// run under. Its own variant: the fix is on that side (install, `claude`
    /// to log in), and the message names it.
    #[error("Claude Code: {0}")]
    ClaudeCode(String),

    /// **`claude` exited before finishing the turn**, with the end of what
    /// it wrote on stderr (`said`), kept whole from a line boundary — the
    /// reader that knows the secrets scrubs it, and only then cuts it for a
    /// sentence. It was cut to 600 characters here, before any scrub, so a
    /// `password=` pair whose name fell before the cut left its value
    /// unmatched (full review 10-21, E 3, probed).
    #[error("Claude Code: `claude` exited ({status}) before finishing the turn{}", said_after(.said))]
    ClaudeExited {
        /// How it exited, as the OS says (`exit status: 1`, a signal).
        status: String,
        /// The end of its stderr, from a line boundary; empty when it said
        /// nothing.
        said: String,
    },
}

/// `said`, as the end of a sentence: ` — said`, or nothing.
fn said_after(said: &str) -> String {
    if said.is_empty() {
        String::new()
    } else {
        format!(" — {said}")
    }
}
