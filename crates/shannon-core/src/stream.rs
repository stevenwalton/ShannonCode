//! Streamed-output protocol shared between `Backend` impls and the TUI.
//!
//! Every backend (canned stub, public API, future OAuth flow) emits a
//! sequence of `StreamMsg` values over an `UnboundedSender` and the
//! TUI's event loop drains them into the output buffer. The protocol
//! is deliberately small — three variants — because the producers and
//! consumers are otherwise unrelated and we want a stable seam.
//!
//! `Error` is new in M1's real-backend work: a stub never fails, but a
//! network round-trip absolutely will. The variant carries a
//! categorized [`BackendError`] (not a bare string) so a consumer can
//! match on the failure mode — the TUI inlines its `Display` form for
//! now, but M2's OAuth backend needs to distinguish auth from
//! rate-limit from transport.

use serde::{Deserialize, Serialize};

use crate::error::BackendError;

/// **What one turn cost, in the two intervals anybody ever needs.**
///
/// A turn can take several tool round-trips, and each round is a separate API
/// call whose prompt *already contains every earlier round*. So there are two
/// honest answers to "how many tokens is this turn", and they are answers to
/// different questions:
///
/// - [`whole`](Self::whole) — every request summed. What it **cost**.
/// - [`final_round`](Self::final_round) — the last request alone. What the
///   context window **holds**, and therefore what the next request re-sends.
///
/// They are equal for a single-round turn, which is nearly every turn — and
/// that is exactly why this type exists. The two travelled as two positional
/// `Usage` arguments through nine signatures, so passing the same value twice,
/// or the two the wrong way round, compiled and passed. It happened three
/// times: `Billing::record` was given the sum for both and read 2.24× too
/// full; `render_session` did the same on the replay path a commit after that
/// was fixed; and `compose_continuation`, which seals its turn by hand,
/// recorded neither.
///
/// One value with two named fields cannot be mis-paired by a caller, and a new
/// consumer gets both or neither. The rule this encodes is the one the codebase
/// keeps re-learning: **two quantities that coincide in the common case still
/// need two names, and the names have to travel together**
/// (`[[project_shannon_n_equals_two]]`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnCost {
    /// Every request this turn made, summed — the bill.
    pub whole: Usage,
    /// Its **final** request alone — the context window's fill, since that
    /// prompt already contains every round before it.
    pub final_round: Usage,
}

impl TurnCost {
    /// A turn that took one request, where the two intervals coincide.
    ///
    /// Named, so a caller that means "these really are the same call" says so,
    /// and a caller that has two different values cannot reach for it by
    /// accident. Replay uses it for a turn stored before `final_round` was
    /// recorded, and every single-round test fixture uses it.
    pub fn single_round(usage: Usage) -> Self {
        Self {
            whole: usage,
            final_round: usage,
        }
    }
}

/// Token counts reported by the backend for a turn. The Anthropic stream
/// reports `input_tokens` on `message_start` and the (cumulative) turn
/// `output_tokens` on `message_delta`, so a single event usually carries one
/// field and leaves the other `0`; the consumer takes the per-field maximum
/// over a turn (the values are cumulative, not incremental). Plain `u32`s —
/// the API never reports a turn beyond `u32` tokens.
///
/// **`input_tokens` is not the prompt size.** Once prompt caching is on, the
/// wire splits one number into three: `input_tokens` counts only what was
/// neither read from nor written to the cache. So the honest "how big was this
/// prompt" is [`prompt_tokens`](Self::prompt_tokens), and anything rendering a
/// bare `input_tokens` as *the* input silently started under-reporting the
/// moment the first cache hit landed. The three are disjoint, which is what
/// makes summing them correct.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// Prompt tokens WRITTEN into the cache this turn — the incremental part
    /// of the prefix that had no entry yet. Billed above the base input rate
    /// (1.25× at the 5-minute TTL), which is why it is worth seeing separately
    /// rather than folded into the total.
    ///
    /// `skip_serializing_if` keeps a turn taken with caching off byte-identical
    /// to one persisted before this field existed — the store's shape does not
    /// move for a feature that did not run.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_creation_input_tokens: u32,
    /// Prompt tokens READ from the cache this turn — the prefix that was
    /// already there, billed at 0.1×. This is the number that says caching is
    /// working.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_read_input_tokens: u32,
}

/// `skip_serializing_if` predicate for the two cache counts (serde hands the
/// predicate a reference to the field).
fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Usage {
    /// Fold another reading into this one — the per-field max, since a turn's
    /// `input`/`output` arrive on different events and each is cumulative.
    pub fn merge(&mut self, other: Usage) {
        self.input_tokens = self.input_tokens.max(other.input_tokens);
        self.output_tokens = self.output_tokens.max(other.output_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .max(other.cache_creation_input_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .max(other.cache_read_input_tokens);
    }

    /// Add a completed turn's totals into a running session sum (saturating —
    /// a session never realistically overflows `u32`, but stay panic-free).
    pub fn add_turn(&mut self, turn: Usage) {
        self.input_tokens = self.input_tokens.saturating_add(turn.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(turn.output_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .saturating_add(turn.cache_creation_input_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(turn.cache_read_input_tokens);
    }

    /// **The whole prompt**: uncached input plus everything read from, and
    /// written to, the cache. The three wire counts are disjoint, so this is
    /// the number a user means by "how much context did that turn carry".
    pub fn prompt_tokens(&self) -> u32 {
        self.input_tokens
            .saturating_add(self.cache_creation_input_tokens)
            .saturating_add(self.cache_read_input_tokens)
    }

    /// What fraction of the prompt came from the cache, as a percentage
    /// (0–100), or `None` when nothing was cached — the one glanceable number
    /// that says whether caching is paying for itself. `None` rather than `0`
    /// so a surface can omit the segment entirely instead of advertising a
    /// feature that did not engage.
    pub fn cache_hit_percent(&self) -> Option<u32> {
        let prompt = self.prompt_tokens();
        if self.cache_read_input_tokens == 0 || prompt == 0 {
            return None;
        }
        // u64 so the multiply cannot overflow a large session total.
        Some(((self.cache_read_input_tokens as u64 * 100) / prompt as u64) as u32)
    }
}

/// Why a turn stopped where it did — the API's `stop_reason` vocabulary plus
/// Shannon's own [`Interrupted`](Self::Interrupted). Recorded (unhashed, like
/// [`Usage`]) into a sealed turn's provenance: without it, "continue where we
/// left off" can't even know a turn is continuable, and a truncated reply is
/// indistinguishable from a complete one. The [`Other`](Self::Other) catch-all
/// keeps persisted logs and the stream parser tolerant of stop reasons this
/// build doesn't know (forward compat, the same posture as unknown config keys).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model finished its reply naturally.
    EndTurn,
    /// The reply was cut by the `max_tokens` output cap — continuable.
    MaxTokens,
    /// A configured stop sequence ended the reply.
    StopSequence,
    /// The model wants a tool result before continuing (the §7 loop).
    ToolUse,
    /// A server-side tool loop paused; resumable by re-sending (§7).
    PauseTurn,
    /// The model or its safety layer declined to continue.
    Refusal,
    /// The context window itself was exhausted (distinct from `max_tokens`).
    ModelContextWindowExceeded,
    /// **Shannon's own**: the stream failed or was cancelled mid-turn and the
    /// billed partial was sealed rather than destroyed — continuable.
    Interrupted,
    /// A stop reason this build doesn't know. Serializes as the raw string,
    /// so nothing is lost round-tripping through the store.
    #[serde(untagged)]
    Other(String),
}

impl StopReason {
    /// From the wire string of the SSE `message_delta.delta.stop_reason`.
    pub fn from_wire(s: &str) -> Self {
        match s {
            "end_turn" => Self::EndTurn,
            "max_tokens" => Self::MaxTokens,
            "stop_sequence" => Self::StopSequence,
            "tool_use" => Self::ToolUse,
            "pause_turn" => Self::PauseTurn,
            "refusal" => Self::Refusal,
            "model_context_window_exceeded" => Self::ModelContextWindowExceeded,
            other => Self::Other(other.to_string()),
        }
    }

    /// Whether `:continue` can pick a turn with this stop reason up where it
    /// left off. `PauseTurn` becomes continuable when the §7 tool loop lands
    /// (its resume shape is re-sending, not text continuation).
    pub fn is_continuable(&self) -> bool {
        matches!(self, Self::MaxTokens | Self::Interrupted)
    }
}

impl std::fmt::Display for StopReason {
    /// The wire-style name (`max_tokens`, `interrupted`) — what the inspector
    /// shows. `Other` displays its raw string.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::EndTurn => "end_turn",
            Self::MaxTokens => "max_tokens",
            Self::StopSequence => "stop_sequence",
            Self::ToolUse => "tool_use",
            Self::PauseTurn => "pause_turn",
            Self::Refusal => "refusal",
            Self::ModelContextWindowExceeded => "model_context_window_exceeded",
            Self::Interrupted => "interrupted",
            Self::Other(s) => s,
        };
        f.write_str(s)
    }
}

/// One unit of streamed output from a `Backend` to the TUI's event loop.
///
/// Not `Eq` — [`StreamMsg::ToolUse`] carries arbitrary JSON input.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamMsg {
    /// Text to append to the in-progress assistant message. May carry
    /// one char or many; the consumer joins onto the current line and
    /// splits on embedded `'\n'` as needed.
    Chunk(String),
    /// The assistant's turn is finished cleanly. The TUI seals the
    /// in-progress message with a blank separator and advances the
    /// conversation state.
    Complete,
    /// The backend failed mid-turn. Carries a categorized error so
    /// consumers can branch on the failure mode; the TUI renders its
    /// `Display` inline so the user knows their submission did not
    /// produce a normal response.
    Error(BackendError),
    /// Token usage for the in-progress turn (input on `message_start`, output
    /// on `message_delta`). The consumer merges readings within a turn and
    /// sums turns into a per-session total. Advisory — never gates the turn.
    Usage(Usage),
    /// **One request of the turn ended and the next began** — from a backend
    /// that runs its own tool rounds inside one turn (Claude Code), so the
    /// consumer closes the last request's books exactly as it does between
    /// its own rounds: `Usage` readings before this are one request, those
    /// after another. Without it a whole agentic turn read as ONE request,
    /// and "how full is the window" summed every round's prompt.
    NextRequest,
    /// Why the turn is ending (from `message_delta.delta.stop_reason`,
    /// shortly before `Complete`). Advisory, like `Usage` — recorded into the
    /// turn's provenance so truncation is visible and `:continue` has ground
    /// truth to stand on.
    Stopped(StopReason),
    /// A COMPLETE `tool_use` block the model emitted — the model asking to run a
    /// tool. Assembled by the backend from the three wire events that carry it
    /// (`content_block_start` → `input_json_delta`s → `content_block_stop`), so
    /// the consumer receives one whole request rather than fragments. Always a
    /// [`Block::ToolUse`](crate::Block::ToolUse). The turn then stops with [`StopReason::ToolUse`]; the
    /// consumer gates it (permission) and replies with a `tool_result`.
    ToolUse(crate::Block),
    /// **Thinking text as it streams** — for the screen. The block it belongs
    /// to arrives whole as [`Thought`](Self::Thought) when it closes. Never
    /// arrives under `display: "omitted"`, whose blocks are empty.
    ThinkingChunk(String),
    /// **A COMPLETE thinking block** — a [`Block::Thinking`](crate::Block::Thinking)
    /// with its signature, or a [`Block::RedactedThinking`](crate::Block::RedactedThinking)
    /// — to keep with the reply and send back unchanged
    /// (`Conversation::push_thinking`). Assembled by the backend, as a
    /// [`ToolUse`](Self::ToolUse) is.
    Thought(crate::Block),
    /// **Thinking the API dropped from this request**, one reason a block
    /// (the response's `input_transformations`): what the model answered
    /// without, said rather than lost in silence.
    ThinkingDropped(Vec<Dropped>),
    /// **A provider that runs its own tools asks whether it may run this
    /// one** — a `claude -p` turn, through Shannon's PreToolUse hook
    /// (`docs/DesignDocs/claude-code-backend.md`). The consumer gates it as it
    /// gates a [`ToolUse`](Self::ToolUse), but does NOT run it: it answers
    /// through [`Backend::answer_tool`](crate::backend::Backend::answer_tool),
    /// and the provider runs it — or refuses, as told. Unanswered, it is
    /// refused when the turn ends.
    ToolAsk(ToolQuestion),
    /// **The provider stopped waiting for a [`ToolAsk`](Self::ToolAsk)** —
    /// its hook is gone (Claude Code killed it: a sibling call failed, its own
    /// timeout). An answer now reaches nobody, so the question must leave the
    /// screen rather than wait for one: the question, as it was asked.
    ToolWithdrawn(AskId),
    /// **A question the provider's gate refused unasked**: its call was
    /// never named in the turn's output — a background agent's, whose calls
    /// Claude Code never shows, or a made-up one from a process that found
    /// the gate's socket (B F2, property 4). Never put to the pane, so said:
    /// the call, by its id and its tool's name, as the question gave them.
    ToolUnnamed { id: String, name: String },
    /// **A tool call a provider ran itself, and what came back** — a record,
    /// to show and to keep, never to gate or run: the gate already answered
    /// the [`ToolAsk`](Self::ToolAsk). `call` is a `Block::ToolUse`, and
    /// `result` its `Block::ToolResult`.
    ToolRecord {
        call: crate::Block,
        result: crate::Block,
    },
    /// The provider's own session for this conversation — kept, so the next
    /// turn continues it (`claude -p --resume`).
    ProviderSession(String),
    /// **The MCP servers a provider started for this turn**, and how each
    /// stands — Claude Code's `init` names them, so Shannon lists them
    /// without reading Claude Code's config.
    ProviderServers(Vec<McpServer>),
    /// **The provider compacted its own context** — Claude Code's
    /// `compact_boundary`: its model works from a summary from here on, while
    /// Shannon's history keeps every turn. `auto` when it did so unasked
    /// (`[claude_code] auto_compact`); `before`, the tokens it held then.
    /// Dropped in silence until 10-15, so the two histories could part with
    /// nothing said.
    ProviderCompacted { auto: bool, before: u32 },
}

/// **Why the API dropped a thinking block it was sent** — the `reason` of an
/// `input_transformations` entry of type `thinking_dropped`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dropped {
    /// `prefix_binding_mismatch`: the conversation before it changed.
    Prefix,
    /// `model_binding_mismatch`: the model answering now cannot read it.
    Model,
    /// `organization_binding_mismatch`: another account's block.
    Account,
    /// A reason this build does not know, as the API wrote it.
    Other(String),
}

impl Dropped {
    /// `reason` as the API writes it.
    pub fn from_wire(reason: &str) -> Dropped {
        match reason {
            "prefix_binding_mismatch" => Dropped::Prefix,
            "model_binding_mismatch" => Dropped::Model,
            "organization_binding_mismatch" => Dropped::Account,
            other => Dropped::Other(other.to_string()),
        }
    }
}

/// **What a provider asks** ([`StreamMsg::ToolAsk`]): one call, as its hook
/// gave it, and when the hook stops waiting for the answer. One value, so
/// the consumer admits a question whole, as it arrived.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolQuestion {
    /// The call's `tool_use_id`, as the asker said it — for the record,
    /// never to answer by ([`AskId`]).
    pub id: String,
    /// The provider's own name for the tool (`Bash`, `Read`, …).
    pub name: String,
    /// Its input, as given.
    pub input: serde_json::Value,
    /// The directory the provider would run it in, as it says (Claude
    /// Code's hook input `cwd`) — `None` when it did not say. The gate
    /// judges a line from where Shannon started, so a call that would run
    /// anywhere else is refused (review 21, Reviewer B).
    pub cwd: Option<String>,
    /// **Which question this is** — what the answer is sent by, never `id`.
    pub ask: AskId,
    /// **When the asker stops waiting for the answer**, on this process's
    /// monotonic clock: Claude Code's hook refuses at its own deadline
    /// (`claude_code::HOOK_DEADLINE` after it asked) — a no, never a yes —
    /// and withdraws the question ([`StreamMsg::ToolWithdrawn`]). `None`: as
    /// long as the turn lives.
    ///
    /// **An instant, not a span**: it was how long was left when this was
    /// sent, made an instant when the consumer read it — and the consumer's
    /// loop is blocked while `$EDITOR` runs, so ten minutes reviewing a
    /// summary told `:ls` ten minutes more than the hook would wait (review
    /// stop 8, A 6).
    pub answer_by: Option<std::time::Instant>,
}

/// **Which question a [`StreamMsg::ToolAsk`] is** — a serial the backend
/// minted for it, and the call's id as the asker said it.
///
/// **The serial is what an answer goes by**
/// ([`Backend::answer_tool`](crate::backend::Backend::answer_tool)); the id
/// is kept for the record, never to route by. A hook question's
/// `tool_use_id` is the asker's to say, and any local process holding the
/// gate's socket can ask — Claude Code's tool shells inherit its path. Keyed
/// by it, a second question for an id overwrote the first's way back, and an
/// answer given to one call reached whichever had asked last: under two
/// turns, another session's (review of `buffers.md`, 10-15, Reviewer B F2).
/// Minted from one process-wide count, so no two questions share one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AskId {
    serial: u64,
    tool_use_id: String,
}

impl AskId {
    /// A question never asked before, about the call `tool_use_id`. The
    /// crate's own: a question's serial comes from the backend that asked
    /// it, never from its consumer (review stop 7, Reviewer E 9).
    pub(crate) fn mint(tool_use_id: &str) -> AskId {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        AskId {
            serial: NEXT.fetch_add(1, Ordering::Relaxed),
            tool_use_id: tool_use_id.to_string(),
        }
    }

    /// A question a test names: `serial` is the test's to keep distinct.
    #[cfg(any(test, feature = "test-util"))]
    pub fn for_test(tool_use_id: &str, serial: u64) -> AskId {
        AskId {
            serial,
            tool_use_id: tool_use_id.to_string(),
        }
    }

    /// The serial an answer is sent by.
    pub fn serial(&self) -> u64 {
        self.serial
    }

    /// The call it asks about, as the asker named it.
    pub fn tool_use_id(&self) -> &str {
        &self.tool_use_id
    }
}

/// One MCP server a provider runs: its name, as configured, and its status
/// (`connected`, `failed`, `needs-auth`, `pending`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub status: String,
}

impl McpServer {
    /// **A server's name as Claude Code's MCP documentation says it is
    /// written into its tools' names**: any character outside `A-Za-z0-9_-`
    /// replaced with `_`.
    pub fn normal(name: &str) -> String {
        name.chars()
            .map(
                |c| match c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    true => c,
                    false => '_',
                },
            )
            .collect()
    }

    /// **Whether two names, written so, are one server** — compared past
    /// what the documentation leaves open: how many `_` a run of replaced
    /// characters leaves, and whether a name's ends keep theirs. So each run
    /// of `_` reads as one and the ends' are dropped; a name that is nothing
    /// else names no server. Until 11-07 Shannon assumed answers there; now
    /// it assumes none, and
    /// the wider match only ever refuses more — a `[mcp] disabled` entry
    /// meets every spelling its server's tools could carry, where a wrong
    /// prediction passed them.
    ///
    /// The one answer to "is it that server", for the gate ([`owns`](Self::owns))
    /// and the screen ([`named_by`](Self::named_by)): the screen matched an
    /// entry by text and the gate by its normal name, so `my_server` refused
    /// `my server`'s tools while its row said `connected` (review 29,
    /// Reviewer C: D1).
    fn same(a: &str, b: &str) -> bool {
        fn words(s: &str) -> impl Iterator<Item = &str> {
            s.split('_').filter(|w| !w.is_empty())
        }
        words(a).next().is_some() && words(a).eq(words(b))
    }

    /// **Does `entry` — a `[mcp] disabled` entry, trimmed as every list's
    /// entry is — name the server Claude Code calls `name`?** An empty one
    /// names none: it named every call that names no server (a listing of
    /// all servers' resources).
    pub fn named_by(name: &str, entry: &str) -> bool {
        Self::same(&Self::normal(entry.trim()), &Self::normal(name))
    }

    /// **Whether `tool` is one of the server `entry` names** —
    /// `mcp__<server>__<tool>`, the server's name [`normal`](Self::normal);
    /// a server's prompt is named so too (`/mcp__<server>__<prompt>`). A
    /// server's name can hold `__` itself, so every place it could end is
    /// tried, and any that names the entry's server makes the tool its —
    /// the wider reading, refusing more, never less.
    pub fn owns(entry: &str, tool: &str) -> bool {
        let wanted = Self::normal(entry.trim());
        let Some(rest) = tool.strip_prefix("mcp__") else {
            return false;
        };
        (rest.char_indices())
            .filter(|&(at, _)| rest[at..].starts_with("__"))
            .any(|(at, _)| Self::same(&wanted, &rest[..at]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_reason_round_trips_through_serde_including_unknowns() {
        // Known variants serialize as the wire strings (what the store holds).
        for (reason, wire) in [
            (StopReason::EndTurn, "\"end_turn\""),
            (StopReason::MaxTokens, "\"max_tokens\""),
            (StopReason::Interrupted, "\"interrupted\""),
        ] {
            assert_eq!(serde_json::to_string(&reason).unwrap(), wire);
            assert_eq!(
                serde_json::from_str::<StopReason>(wire).unwrap(),
                reason,
                "{wire}"
            );
        }
        // An unknown reason survives a round trip as `Other` — a log written
        // by a future build still deserializes here, losing nothing.
        let other = StopReason::Other("novel_reason".into());
        let json = serde_json::to_string(&other).unwrap();
        assert_eq!(json, "\"novel_reason\"");
        assert_eq!(serde_json::from_str::<StopReason>(&json).unwrap(), other);
        // **Every known variant, all three spellings** — `from_wire`,
        // `Display` and serde, each held to one table. `wire` is a `match`
        // with no `_`, so a variant added is a compile error until it is
        // named; this checked three of eight, and a misspelt `pause_turn` in
        // `from_wire` passed (review stop 12, T9).
        use StopReason as R;
        let wire = |r: &R| match r {
            R::EndTurn => Some("end_turn"),
            R::MaxTokens => Some("max_tokens"),
            R::StopSequence => Some("stop_sequence"),
            R::ToolUse => Some("tool_use"),
            R::PauseTurn => Some("pause_turn"),
            R::Refusal => Some("refusal"),
            R::ModelContextWindowExceeded => Some("model_context_window_exceeded"),
            R::Interrupted => Some("interrupted"),
            R::Other(_) => None,
        };
        for reason in [
            R::EndTurn,
            R::MaxTokens,
            R::StopSequence,
            R::ToolUse,
            R::PauseTurn,
            R::Refusal,
            R::ModelContextWindowExceeded,
            R::Interrupted,
        ] {
            let name = wire(&reason).expect("a known variant");
            assert_eq!(reason.to_string(), name, "Display");
            assert_eq!(
                serde_json::to_string(&reason).unwrap(),
                format!("\"{name}\"")
            );
            let json = format!("\"{name}\"");
            assert_eq!(serde_json::from_str::<R>(&json).unwrap(), reason, "{json}");
            // The wire never says `interrupted` — Shannon's own, sealed by
            // us — so it reads as an unknown there, and only there.
            let read = StopReason::from_wire(name);
            match reason {
                R::Interrupted => assert_eq!(read, R::Other(name.into())),
                _ => assert_eq!(read, reason, "from_wire({name})"),
            }
        }
        // from_wire agrees with serde on both sides of the split.
        assert_eq!(StopReason::from_wire("max_tokens"), StopReason::MaxTokens);
        assert_eq!(
            StopReason::from_wire("novel_reason"),
            StopReason::Other("novel_reason".into())
        );
    }

    /// **A server owns exactly its tools** — `mcp__<name>__…`, the name as
    /// Claude Code normalises it; a server whose name only begins the same
    /// owns none of them.
    #[test]
    fn a_server_owns_exactly_its_tools() {
        assert!(McpServer::owns("github", "mcp__github__create_issue"));
        assert!(!McpServer::owns("github", "mcp__github2__create_issue"));
        assert!(!McpServer::owns("git", "mcp__github__create_issue"));
        assert!(!McpServer::owns("github", "Bash"));
        assert!(McpServer::owns("my server.v2", "mcp__my_server_v2__x"));
        assert!(
            McpServer::owns("my-server_1", "mcp__my-server_1__x"),
            "- and _ kept"
        );
        assert!(McpServer::owns("my  server", "mcp__my_server__search"));
    }

    /// **A name is written as documented and compared past what is not**
    /// (review 29; 11-07): one `_` for each character outside `A-Za-z0-9_-`;
    /// how many `_` a run leaves, and whether the ends keep theirs, are not
    /// asked — a longer run or a trimmed end names the same server either
    /// way. An entry of yours trimmed; an entry and a
    /// server are one when they agree so — the screen's question and the
    /// gate's; words stay words.
    #[test]
    fn a_name_is_written_as_documented_and_compared_past_what_is_not() {
        assert_eq!(McpServer::normal("a😀b"), "a_b", "one character, one `_`");
        assert_eq!(McpServer::normal("aéb"), "a_b");
        assert_eq!(McpServer::normal("my server!"), "my_server_");
        assert!(
            McpServer::owns("my server!", "mcp__my_server__x"),
            "an end's `_` kept or not, the same server"
        );
        assert!(McpServer::owns("my server!", "mcp__my_server___x"));
        assert!(
            McpServer::owns("a b", "mcp__a__b__x"),
            "a server's name holding `__`: every end is tried"
        );
        assert!(
            McpServer::owns(" github ", "mcp__github__x"),
            "yours trimmed"
        );
        assert!(McpServer::named_by("my server", " my_server "));
        assert!(McpServer::named_by("my  server", "my server"));
        assert!(!McpServer::named_by("my server", "my-server"));
        assert!(!McpServer::named_by("ab", "a b"), "words stay words");
        assert!(!McpServer::named_by("", " "), "an empty entry names none");
        assert!(!McpServer::named_by("?", "!!"), "nor one that is only `_`");
    }

    #[test]
    fn continuable_is_exactly_truncation_and_interruption() {
        assert!(StopReason::MaxTokens.is_continuable());
        assert!(StopReason::Interrupted.is_continuable());
        for r in [
            StopReason::EndTurn,
            StopReason::StopSequence,
            StopReason::ToolUse,
            StopReason::PauseTurn,
            StopReason::Refusal,
            StopReason::ModelContextWindowExceeded,
            StopReason::Other("x".into()),
        ] {
            assert!(!r.is_continuable(), "{r} must not be continuable");
        }
    }
}

#[cfg(test)]
mod cache_usage_tests {
    use super::*;

    /// **The three wire counts are disjoint, so the prompt is their sum.**
    ///
    /// This is the API's own arithmetic: `input_tokens` holds only what fell
    /// after the last cache breakpoint, which for our rolling tail breakpoint is
    /// close to nothing. Anything rendering `input_tokens` as *the* input starts
    /// under-reporting the moment a cache hit lands, and does so silently — the
    /// number stays plausible, it is just the wrong one.
    #[test]
    fn the_prompt_is_all_three_counts() {
        let u = Usage {
            input_tokens: 12,
            output_tokens: 40,
            cache_creation_input_tokens: 134,
            cache_read_input_tokens: 1100,
        };
        assert_eq!(u.prompt_tokens(), 1246);
        // Not the wire field, which is the whole point.
        assert_ne!(u.prompt_tokens(), u.input_tokens);
    }

    /// The hit rate is `read / prompt`, and is `None` — not `Some(0)` — when
    /// nothing was read, so a surface can drop the segment rather than advertise
    /// a feature that never engaged. A prompt below the model's minimum
    /// cacheable length is exactly that case, and it is the common one for short
    /// sessions.
    #[test]
    fn the_hit_rate_is_absent_rather_than_zero() {
        let cold = Usage {
            input_tokens: 500,
            output_tokens: 40,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        assert_eq!(cold.cache_hit_percent(), None);
        // A first request writes without reading: still nothing to show.
        let written = Usage {
            cache_creation_input_tokens: 900,
            ..cold
        };
        assert_eq!(written.cache_hit_percent(), None);
        let warm = Usage {
            input_tokens: 10,
            output_tokens: 40,
            cache_creation_input_tokens: 90,
            cache_read_input_tokens: 900,
        };
        assert_eq!(warm.cache_hit_percent(), Some(90));
        assert_eq!(
            Usage::default().cache_hit_percent(),
            None,
            "no divide by zero"
        );
    }

    /// `merge` folds two readings of ONE request (the counts arrive on
    /// different SSE events and each is cumulative) — per-field max. `add_turn`
    /// folds separate requests into a session total — per-field sum. The cache
    /// counts must follow whichever fold their neighbours do, or a session
    /// total reports the last turn's cache and a single turn reports the sum of
    /// two readings of itself.
    #[test]
    fn the_cache_counts_fold_the_way_their_neighbours_do() {
        let mut turn = Usage {
            input_tokens: 12,
            output_tokens: 0,
            cache_creation_input_tokens: 134,
            cache_read_input_tokens: 1100,
        };
        // `message_delta` re-reports the same prompt counts and adds output.
        turn.merge(Usage {
            input_tokens: 12,
            output_tokens: 40,
            cache_creation_input_tokens: 134,
            cache_read_input_tokens: 1100,
        });
        assert_eq!(turn.cache_read_input_tokens, 1100, "max, not sum");
        assert_eq!(turn.cache_creation_input_tokens, 134, "max, not sum");

        let mut session = turn;
        session.add_turn(turn);
        assert_eq!(session.cache_read_input_tokens, 2200, "sum, not max");
        assert_eq!(session.cache_creation_input_tokens, 268, "sum, not max");
        assert_eq!(session.prompt_tokens(), 2492);
    }

    /// **A turn taken with caching off persists exactly as it did before this
    /// field existed**, and one persisted before it existed still parses. The
    /// store holds `Usage` inside every turn's provenance, so a shape change
    /// here rewrites history's format for a feature that did not run.
    #[test]
    fn the_stored_shape_does_not_move_for_an_uncached_turn() {
        let plain = Usage {
            input_tokens: 100,
            output_tokens: 40,
            ..Usage::default()
        };
        assert_eq!(
            serde_json::to_value(plain).unwrap(),
            serde_json::json!({"input_tokens": 100, "output_tokens": 40}),
            "no cache keys on a turn that did not cache",
        );
        let old: Usage =
            serde_json::from_str(r#"{"input_tokens":100,"output_tokens":40}"#).unwrap();
        assert_eq!(old, plain, "a log written before the fields existed parses");

        // …and a cached turn does carry them, or the readout has nothing to read.
        let cached = Usage {
            cache_read_input_tokens: 900,
            ..plain
        };
        assert_eq!(
            serde_json::to_value(cached).unwrap()["cache_read_input_tokens"],
            900,
        );
    }
}
