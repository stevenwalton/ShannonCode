//! Shannon Code — orchestrator core.
//!
//! Hosts the chat-protocol surface the TUI binary (`shannon-tui`) and any
//! future frontend consume:
//!
//! - `message` — neutral conversation types (`Message`, `Role`). Backend
//!   implementations translate to and from their provider-specific shapes
//!   without leaking into the rest of the codebase.
//! - `conversation` — `Conversation`: the transcript + per-turn
//!   lifecycle (begin / commit / fail) that keeps history role-
//!   alternating. The seed of §1's conversation-graph model; the TUI
//!   owns one and updates its view in tandem rather than owning the
//!   transcript itself.
//! - `graph` — the conversation/history graph (design.md §1): content-
//!   addressed `Turn`s, `Episode`s (ordered turns + config + parent edge),
//!   and the in-memory `Graph` with one recursive `Node` abstraction that
//!   walks every level of the fractal structure. Grows *behind*
//!   `conversation`, which stays the lifecycle façade, so no frontend
//!   changes until a slice is ready to consume.
//! - `store` — the `Store` trait (design.md §1.3.1): swappable persistence
//!   for the graph. `MemoryStore` (tests / ephemeral) and `JsonlStore`
//!   (append-only, `cat`/`tail -f`-able on disk) are held to one shared
//!   contract. The in-memory `Graph` is rebuilt from a `Store` at startup,
//!   never read per frame.
//! - `stream` — `StreamMsg`, the chunked-output channel protocol that
//!   the TUI's event loop already speaks. Lives here (not in `shannon-tui`)
//!   so `Backend` impls can produce it without depending on the TUI.
//! - `error` — `BackendError`, the categorized failure type carried by
//!   `StreamMsg::Error` so consumers branch on the mode, not a string.
//! - `secret` — `Secret`, a redacting credential wrapper.
//! - `config` — `Config`, the user-configuration substrate (sparse TOML
//!   over coded defaults, never written back). Domain data so any
//!   frontend reads it; TUI-only knobs live under `tui` as plain data.
//! - `backend` — the `Backend` trait, the `SendRequest` input struct,
//!   and its implementations: `CannedStub` (offline demo / no-key
//!   fallback), `AnthropicApi` (real Claude via the public Messages API,
//!   on an API key) and `ClaudeCode` (the user's own `claude`). No
//!   Claude.ai subscription login: Anthropic's terms do not allow one in a
//!   third-party app.
//!
//! As §1 of `docs/DesignDocs/design.md` lands, the conversation/
//! persistence layer grows *behind* `conversation` and `backend`; the
//! TUI consumes them through this stable surface rather than reaching
//! around them.

pub mod backend;
pub mod claude_import;
pub mod config;
pub mod conversation;
pub mod error;
pub mod exec;
#[cfg(any(test, feature = "test-util"))]
#[rustfmt::skip]
pub mod fakes;
pub mod files;
pub mod graph;
pub mod hook;
pub mod knowledge;
pub mod message;
mod pattern;
pub mod permission;
pub mod scratch;
pub mod secret;
pub mod secrets;
pub mod shell;
pub mod store;
pub mod stream;
pub mod text;
pub mod when;
pub mod window;

pub use backend::{Backend, ProviderEnv, SendRequest, ThinkingAsk, ToolDef};
pub use config::{
    AgentsConfig, AliasSpec, AuditLevel, ClaudeFiles, Config, ContinueConfig, ContinueStrategy,
    CopySecrets, DEFAULT_CONTINUE_INSTRUCTION, DEFAULT_SQUASH_INSTRUCTION, DEFAULT_TOML,
    GraphChars, GraphIds, GraphSort, GraphStyle, MemoryConfig, MemoryMode, MergeConfig, OnLimit,
    ProjectName, ResumeMode, SkillsConfig, StartMode, StatusConfig, ThinkingDisplay, TokenCounts,
};
pub use conversation::Conversation;
pub use error::BackendError;
/// A test double, behind `test-util` — see the feature comment in `Cargo.toml`.
#[cfg(any(test, feature = "test-util"))]
pub use exec::HangingExecutor;
pub use exec::{
    ExecConfig, ExecOutcome, Executor, LeftBehind, Leftover, RealExecutor, RecordingExecutor,
};
pub use graph::{
    EdgeKind, Effort, Episode, EpisodeId, Graph, Inherited, Label, Mark, MergeError,
    MergeStraightIn, MergeStrategy, MessageOutline, Node, OUTLINE_CHARS, Occurrence, Origin, Pick,
    Piece, Placed, Prepend, Provenance, ReRoot, SHORT_ID_LEN, Segment, Squash, SquashMerge, Turn,
    TurnConfig, TurnId, Whose, outline_of, short_hex,
};
pub use message::{
    BASH_TOOL, Block, ContextKind, MARK_TOOL, Message, Role, SKILL_TOOL, from_editable,
    from_editable_turns, to_editable,
};
pub use secret::Secret;
#[cfg(any(test, feature = "test-util"))]
pub use secrets::MemoryVault;
pub use secrets::{
    Alias, Aliases, ENV_SOURCE, FileVault, KEPT_SOURCE, Kept, MARKED_SOURCE, NoVault, PathPolicy,
    Redaction, ResolveAt, Restored, SecretsConfig, Unmasked, Vault, VaultError, classify_kind,
    redact, redact_command, redact_command_shown, redact_json, redact_json_shown, redact_shown,
    secret_handles_in,
};
pub use store::{
    JsonlStore, LoadedEpisode, MemoryStore, Store, StoreError, StoreResult, hydrate, hydrate_turns,
};
pub use stream::{AskId, Dropped, McpServer, StopReason, StreamMsg, ToolQuestion, TurnCost, Usage};
