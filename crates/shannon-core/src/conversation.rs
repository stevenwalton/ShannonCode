//! Conversation transcript + turn lifecycle — the migration façade over §1's graph.
//!
//! Owns the role-alternating message history and the bookkeeping for
//! one in-flight assistant turn. This is design.md §1's conversation/
//! history model wearing a familiar face: the public API (`begin_turn`
//! / `push_chunk` / `commit_turn` / `fail_turn` / `messages`) is
//! unchanged — the TUI's only contract — while *behind* it the
//! content-addressed graph now grows. Internally a `Conversation` holds
//! a single default [`Episode`] inside a [`Graph`]; each committed turn
//! is sealed into a content-addressed [`Turn`] and appended to that
//! episode. The flat `messages` vec is the **mutable working view**
//! handed to the backend; the graph is the **immutable base**. P3 makes
//! `messages` a *projection* of the episode (design.md §4); until then
//! the two are kept in step here.
//!
//! The point is that the *domain invariant* — a valid transcript
//! alternates user / assistant turns, and a failed turn must not leave a
//! dangling user message — lives in `shannon-core`, not in the TUI.
//!
//! Lifecycle of one turn, driven by the event loop:
//!
//! - `begin_turn(user_text)` records the user message and opens an
//!   empty assistant accumulator.
//! - `push_chunk(s)` appends streamed assistant text (zero or more).
//! - `commit_turn()` seals the assistant message into history **and**
//!   into a content-addressed `Turn` on the default episode, OR
//!   `fail_turn()` (the backend errored) drops the user message so
//!   history stays role-alternating — every provider rejects a
//!   trailing user message with no assistant reply followed by
//!   another user message. A failed turn seals nothing.
//!
//! The view layer (the TUI's output pane) is updated *in tandem* by
//! the caller — `Conversation` is the model, not the renderer.

use std::time::SystemTime;

use crate::graph::{Episode, EpisodeId, Follows, Graph, Turn, TurnConfig};
use crate::stream::{StopReason, TurnCost};
use crate::{Block, Message, Role};

/// The in-flight turn between `begin_turn` and its `commit`/`fail`: the
/// user message being answered and the streamed assistant reply so far.
/// At commit the pair is sealed into a content-addressed [`Turn`].
#[derive(Debug, Clone)]
struct InFlight {
    /// **What this turn is actually being produced under**, captured when it
    /// began rather than read from the episode when it is sealed.
    ///
    /// The episode's config is a *preference* (`:model` sets it, it persists);
    /// this is *history*. They differ whenever the app cannot serve the
    /// request — no API key means the stub answers every model — and capturing
    /// it here also means a `:model` mid-flight cannot retro-label a turn it
    /// did not produce.
    config: TurnConfig,
    /// Index in `messages` where this turn begins — **the turn IS
    /// `messages[start..]`**.
    ///
    /// A cursor rather than a second copy of the messages. The previous shape
    /// kept its own `Vec` of finished round-trip messages, so sealing meant
    /// rebuilding a list that had to match the working view, and failing meant
    /// counting how many messages to pop — arithmetic that got the count wrong
    /// and stranded an unanswered `tool_use`, bricking the session. With a
    /// cursor there is no count to keep in step: the sealed turn is literally a
    /// slice of the transcript, so "the graph and the working view agree" is
    /// definitional rather than maintained by hand.
    start: usize,
    /// Accumulated assistant reply (the message currently streaming).
    assistant: String,
    /// Assistant characters this TURN has already shown, across the rounds it
    /// has finished.
    ///
    /// `finish_tool_round` takes `assistant` for each round, so the field above
    /// measures the current round alone — while a turn's billed usage
    /// accumulates across all of them. Comparing the two was comparing
    /// different intervals, and it made `unaccounted_output` fire on every
    /// ordinary multi-round tool turn: the successful agentic path, told it
    /// "may have been cut off" (Reviewer A, reproduced end-to-end). A warning
    /// that fires on success is a warning nobody reads
    /// (`[[project_shannon_permission_failure_mode]]`).
    ///
    /// Its thinking counts too ([`shown_of`]): the transcript draws it.
    shown: usize,
    /// Structured blocks on the streaming assistant message — the `tool_use`
    /// requests it made, and its thinking. Committed with the text as one
    /// message.
    blocks: Vec<Block>,
    /// `tool_result` blocks that **did** arrive before the turn was abandoned.
    ///
    /// A turn can end (a rate limit, `CTRL-C`, a `:child --stop`) after some of
    /// its commands have run and reported. Those answers used to be discarded
    /// with the round, and [`commit_turn_holding`](Conversation::commit_turn_holding) then
    /// synthesized "the turn ended before this ran" for *every* request —
    /// including the ones whose real output was on screen a line above. The
    /// transcript said `⚙ output of: ls` and the persisted conversation told
    /// the model it never ran, so `:continue` handed it a false record
    /// (Reviewer A).
    ///
    /// Kept here rather than pushed straight into `messages`, because a
    /// `tool_result` message may only follow the assistant message that asked —
    /// and that message is still being written.
    answers: Vec<Block>,
}

/// A conversation transcript with single-turn lifecycle tracking, backed
/// by a single default [`Episode`] in the conversation graph.
#[derive(Debug, Clone)]
pub struct Conversation {
    /// The mutable working view: the role-alternating transcript handed
    /// to the backend. Behaves exactly as before the graph landed; the
    /// content-addressed base grows behind it.
    messages: Vec<Message>,
    /// The in-flight turn. `Some` between `begin_turn` and its
    /// `commit_turn` / `fail_turn`; `None` when no turn is in progress.
    in_progress: Option<InFlight>,
    /// The immutable base: sealed [`Turn`]s and the episode DAG. Today a
    /// single default episode; the façade is what lets the full §1 graph
    /// grow without touching the TUI's contract.
    graph: Graph,
    /// The single default episode this façade appends committed turns to.
    episode: EpisodeId,
    /// The config committed turns are sealed under. A documented
    /// placeholder until the resolved model is threaded — see
    /// [`with_config`](Self::with_config).
    config: TurnConfig,
    /// What the in-flight turn has cost, in both intervals, recorded from the
    /// stream ([`record_usage`](Self::record_usage)) and folded into the sealed
    /// turn's provenance at [`commit_turn_holding`](Self::commit_turn_holding). `None` clears
    /// between turns. Metadata, not part of a turn's identity.
    pending_cost: Option<TurnCost>,
    /// Why the in-flight turn is ending, recorded from the stream
    /// ([`record_stop_reason`](Self::record_stop_reason)) and folded into the
    /// sealed turn's provenance at commit — the same lifecycle as
    /// `pending_cost`. Metadata, not identity.
    pending_stop: Option<StopReason>,
}

/// The model a conversation carries when nobody has said which — what
/// [`Conversation::new`] seals under, and the value
/// [`names_a_model`](Conversation::names_a_model) tests for.
pub const UNSPECIFIED_MODEL: &str = "unspecified";

impl Conversation {
    /// A new conversation backed by a single default episode.
    ///
    /// Turns are sealed under a **placeholder** [`TurnConfig`]: the TUI
    /// leaves `SendRequest.model = None` today (the concrete model lives
    /// in the backend instance), so the resolved model is not yet visible
    /// here. [`with_config`](Self::with_config) is the seam to inject it
    /// once that wiring lands; the sealed turns are inert until then
    /// (no persistence or viewer consumes them before P2/P4).
    pub fn new() -> Self {
        Self::with_config(TurnConfig {
            model: UNSPECIFIED_MODEL.into(),
            max_tokens: None,
            effort: None,
        })
    }

    /// Does this conversation's episode actually **name** a model, or is it
    /// carrying the placeholder?
    ///
    /// The question a resume path has to ask before substituting a default. It
    /// lives beside the placeholder because the two are one fact: a caller that
    /// re-spells `"unspecified"` is a second definition waiting to drift
    /// (`[[feedback_rules_rot_by_retyping]]`).
    pub fn names_a_model(&self) -> bool {
        let model = self.config.model.trim();
        !model.is_empty() && model != UNSPECIFIED_MODEL
    }

    /// A new conversation whose committed turns are sealed under `config`, on the
    /// single default episode. `new()` delegates here with a placeholder.
    pub fn with_config(config: TurnConfig) -> Self {
        Self::with_episode(Self::default_episode_id(), config)
    }

    /// A new conversation rooted at an explicit fresh episode `id` — the
    /// `:new`-session path, where the App mints a unique id so the new session
    /// is a distinct episode (the old one stays on disk, reachable via `:graph`).
    pub fn with_episode(id: EpisodeId, config: TurnConfig) -> Self {
        Self::resume(Episode::root(id, config), Vec::new())
    }

    /// Set the **active** model — the one the next committed turn is sealed
    /// under, and the active episode's forward config. The App calls this after
    /// resuming so new turns record the real backend model instead of the
    /// placeholder; already-committed turns keep their own (historical)
    /// provenance. Both the conversation's active config and the episode's
    /// forward config are updated so they stay in sync.
    pub fn set_model(&mut self, model: impl Into<String>) {
        let model = model.into();
        self.config.model = model.clone();
        if let Some(episode) = self.graph.episode_mut(&self.episode) {
            episode.config.model = model;
        }
    }

    /// Set the session's **effort** — its preference, as [`set_model`] sets
    /// its model: `None` follows the run's `[turn] effort`. What a turn is
    /// actually sent is the app's to decide (the model may take none), and is
    /// recorded on the turn, not here.
    ///
    /// [`set_model`]: Self::set_model
    pub fn set_effort(&mut self, effort: Option<crate::Effort>) {
        self.config.effort = effort;
        if let Some(episode) = self.graph.episode_mut(&self.episode) {
            episode.config.effort = effort;
        }
    }

    /// Set the session's **name and description** ([`Label`](crate::Label))
    /// — what its header carries at the next persist; the app re-puts the
    /// header when the session is already on disk, as it does for a model.
    pub fn set_label(&mut self, label: crate::Label) {
        if let Some(episode) = self.graph.episode_mut(&self.episode) {
            episode.label = label;
        }
    }

    /// Set the session's **marks** ([`Mark`](crate::Mark)) — what its header
    /// carries at the next persist, as a label is; the app re-puts the header
    /// when the session is already on disk.
    pub fn set_marks(&mut self, marks: Vec<crate::Mark>) {
        if let Some(episode) = self.graph.episode_mut(&self.episode) {
            episode.marks = Some(marks);
        }
    }

    /// Set **the Claude Code conversation it continues**
    /// ([`Inherited`](crate::Inherited)) — found after the import, for one
    /// made before imports recorded it; the header carries it as a label's.
    pub fn set_inherited(&mut self, inherited: Option<crate::Inherited>) {
        if let Some(episode) = self.graph.episode_mut(&self.episode) {
            episode.inherited = inherited;
        }
    }

    /// The stable id of the single default episode a conversation is built
    /// over when it is given none. The persistence layer
    /// ([`Store`](crate::Store)) keys the on-disk log by this id, so the caller
    /// can `load_episode` it at startup before there is a `Conversation` to
    /// ask — a launch falls back to it when a store has no `HEAD`. A fixed
    /// value ([`EpisodeId::DEFAULT_EPISODE`]), never a derivation: the
    /// derivation may change, and this id must not.
    pub fn default_episode_id() -> EpisodeId {
        EpisodeId::DEFAULT_EPISODE
    }

    /// Rebuild a conversation over an existing episode and its turns — the
    /// startup path: the App loads the episode from a `Store` and hands the
    /// turns here. The working view is the flattened messages of those turns
    /// (oldest first); the graph is seeded with the episode + turns so the
    /// façade picks up exactly where it left off. `with_config` is the
    /// fresh-episode case (no turns).
    pub fn resume(episode: Episode, turns: Vec<Turn>) -> Self {
        let id = episode.id;
        let config = episode.config.clone();
        let messages = turns
            .iter()
            .flat_map(|t| t.messages.iter().cloned())
            .collect();
        let mut graph = Graph::new();
        // Content only: the episode already holds each own turn's occurrence.
        for turn in turns {
            graph.insert_content(turn);
        }
        graph.insert_episode(episode);
        Self {
            messages,
            in_progress: None,
            graph,
            episode: id,
            config,
            pending_cost: None,
            pending_stop: None,
        }
    }

    /// Record the in-flight turn's token usage (from the backend stream). Folded
    /// into the turn's provenance at the next [`commit_turn_holding`](Self::commit_turn_holding).
    /// The App calls this once the turn's usage is known.
    /// **One argument, two intervals** — see [`TurnCost`]. It was two bare
    /// `Usage` values in a row, which a caller could pair wrongly (or pass the
    /// same one twice) with nothing to say so.
    pub fn record_usage(&mut self, cost: TurnCost) {
        self.pending_cost = Some(cost);
    }

    /// Record why the in-flight turn is ending (from the stream's
    /// `message_delta`). Folded into the sealed turn's provenance at the next
    /// [`commit_turn_holding`](Self::commit_turn_holding), exactly like usage.
    pub fn record_stop_reason(&mut self, reason: StopReason) {
        self.pending_stop = Some(reason);
    }

    /// The current (default) episode — its id, config, and turn order. The App
    /// uses it to register the episode with a `Store` on a fresh start. The
    /// invariant that the graph always holds `self.episode` is upheld by every
    /// constructor, so this never panics.
    pub fn episode(&self) -> &Episode {
        self.graph
            .episode(&self.episode)
            .expect("the current episode is always present in the graph")
    }

    /// The episode's tip turn, if any — after [`commit_turn_holding`](Self::commit_turn_holding)
    /// this is the turn just sealed, as its record (content + that occurrence's
    /// provenance). Routed through [`episode`](Self::episode) so a violated graph
    /// invariant surfaces here too (rather than silently returning `None`). Reads
    /// the episode's *own* head; the façade's working view is a single path, so
    /// its own tail is the live end of the session.
    pub fn head_turn(&self) -> Option<Turn> {
        self.own_turns_from(self.episode().own_len().checked_sub(1)?)
            .pop()
    }

    /// The current episode's **own** (divergent) turns, oldest first, as records.
    /// The App consumes exactly this tail: the turns to persist (a branch appends
    /// only what diverged — the shared prefix lives on its ancestors) and the live
    /// turns to overlay onto the graph snapshot.
    pub fn own_turns(&self) -> Vec<Turn> {
        self.own_turns_from(0)
    }

    /// [`own_turns`](Self::own_turns) from own position `from` on — the unsaved
    /// tail a persist appends, without cloning the saved turns before it.
    ///
    /// Each record is one occurrence's facts with the shared content. Resolving
    /// the content by id shouldn't miss — [`commit_turn_holding`](Self::commit_turn_holding)
    /// admits the content in the same step it appends the occurrence — but
    /// [`resume`](Self::resume) is public and trusts its caller to pass every
    /// record. So it STOPS at a missing body rather than skipping it: a persist
    /// writes these positionally, and a skip would file the next turn in the
    /// missing one's place and move the cursor past both (Reviewers A and D,
    /// round 18). Stopping writes fewer, keeps the cursor true, and the next
    /// persist resumes from the gap.
    pub fn own_turns_from(&self, from: usize) -> Vec<Turn> {
        self.episode()
            .own_occurrences()
            .skip(from)
            .map_while(|o| Some(Turn::of(o, self.graph.messages(&o.turn)?.to_vec())))
            .collect()
    }

    /// The transcript so far, oldest first. Does not include the
    /// in-progress assistant reply (which isn't a committed message
    /// yet).
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// A clone of the transcript suitable for handing to a backend.
    /// Includes the user message just recorded by `begin_turn`, since
    /// that is the message the backend is replying to.
    pub fn snapshot(&self) -> Vec<Message> {
        self.messages.clone()
    }

    /// Whether a turn is currently in progress (between `begin_turn`
    /// and its `commit_turn` / `fail_turn`).
    pub fn turn_in_progress(&self) -> bool {
        self.in_progress.is_some()
    }

    /// Begin a turn: record the user message and open the assistant
    /// accumulator. Caller guarantees (via its own single-turn gate)
    /// that no turn is already in progress.
    pub fn begin_turn(&mut self, user_text: impl Into<String>) {
        self.begin_turn_under(user_text, Vec::new(), self.config.clone());
    }

    /// Begin a turn that will be **sealed under `config`**, whatever the
    /// episode's configured model says.
    ///
    /// The two are not the same question and this field's own doc has called
    /// itself "a placeholder until the resolved model is threaded" since it was
    /// written. `Episode.config` records what the session **asks for** — a
    /// preference, which persists and which `:model` sets. A turn's provenance
    /// records what **produced** it, which is history and must not be a wish.
    ///
    /// They diverge whenever the app cannot serve the request: with no API key
    /// the backend is the canned stub for every model, so `:model haiku`
    /// followed by a send used to persist a stub turn labelled `haiku` — and
    /// unlike a status row, an on-disk record is never re-derived, so the lie
    /// outlived the session. Steven: "If they think they're talking to model x
    /// and are actually talking to model y then that's a big failure on our
    /// part."
    ///
    /// The config is a **hash input**, so a turn produced by the stub now has a
    /// different content address from one produced by Haiku. That is the point:
    /// they are not the same turn.
    ///
    /// `told` is what the turn tells the model beyond its prompt
    /// ([`crate::Block::Context`]s, `knowledge::attach`) —
    /// carried on the user message, so the turn records it. A parameter, not
    /// a setter, so every door that starts a turn says what it tells; a turn
    /// that tells nothing passes none.
    pub fn begin_turn_under(
        &mut self,
        user_text: impl Into<String>,
        told: Vec<crate::Block>,
        config: TurnConfig,
    ) {
        debug_assert!(
            self.in_progress.is_none(),
            "a turn was already in flight; starting another discards it"
        );
        let start = self.messages.len();
        let mut message = Message::user(user_text);
        message.blocks = told;
        self.messages.push(message);
        self.in_progress = Some(InFlight {
            config,
            start,
            assistant: String::new(),
            shown: 0,
            blocks: Vec::new(),
            answers: Vec::new(),
        });
    }

    /// Append streamed assistant text to the in-flight turn. A no-op
    /// if called with no turn in progress (defensive — the caller's
    /// stream gating should prevent it).
    pub fn push_chunk(&mut self, chunk: &str) {
        if let Some(draft) = self.in_progress.as_mut() {
            draft.assistant.push_str(chunk);
        }
    }

    /// How much assistant text the in-flight turn has shown so far, in
    /// characters, **across every round of it**. `None` when no turn is in
    /// progress.
    ///
    /// Exists so a caller can compare what the turn is being BILLED for against
    /// what it has to show for it: four turns of a live demo were charged a full
    /// output-token cap and rendered nothing at all, and nothing anywhere
    /// noticed the discrepancy (`[[feedback_no_silent_failure]]`).
    ///
    /// **The whole turn, because that is the interval the bill covers.**
    /// Counting the current round alone compared two different windows and
    /// raised the alarm on every successful tool round. And CHARACTERS, not
    /// bytes: the caller's arithmetic is calibrated in characters, and `len()`
    /// runs two to four times higher on CJK or emoji.
    ///
    /// **Thinking is shown too** (N1b), so its text counts: a turn that
    /// mostly thought, under `display: "summarized"`, drew its summary.
    pub fn draft_len(&self) -> Option<usize> {
        self.in_progress
            .as_ref()
            .map(|d| d.shown + shown_of(&d.assistant, &d.blocks))
    }

    /// **What the turn in flight is being produced under** — the config it
    /// began under and will be sealed under. A later round of the same turn
    /// asks for this, not for what the session prefers now: a `:model`
    /// between two rounds would otherwise seal one turn under one model while
    /// its last round ran on another.
    pub fn in_flight_config(&self) -> Option<&TurnConfig> {
        self.in_progress.as_ref().map(|d| &d.config)
    }

    /// Record a `tool_use` the assistant emitted on the streaming message.
    ///
    /// It must ride on the assistant message itself: the API pairs a
    /// `tool_result` with the `tool_use` it answers, so dropping the block would
    /// leave the reply unattachable.
    pub fn push_tool_use(&mut self, block: Block) {
        if let Some(draft) = self.in_progress.as_mut() {
            draft.blocks.push(block);
        }
    }

    /// Record a thinking block the assistant wrote, whole, where it stands:
    /// after the text of this round so far ([`Block::placed`]). It rides the
    /// message as a call does, and goes back with it unchanged — the API
    /// binds it to everything before it (`docs/DesignDocs/navigation.md` §4).
    pub fn push_thinking(&mut self, block: Block) {
        if let Some(draft) = self.in_progress.as_mut() {
            let at = draft.assistant.len();
            draft.blocks.push(block.placed(at));
        }
    }

    /// Record `tool_result` blocks that arrived but will not complete a round —
    /// the turn is being abandoned. See the `answers` field on the in-flight draft.
    ///
    /// Called on the way OUT, by the teardown that is about to discard the
    /// app's copy: what actually happened is worth more than a tidy uniform
    /// record of what did not.
    pub fn keep_tool_answers(&mut self, results: impl IntoIterator<Item = Block>) {
        if let Some(draft) = self.in_progress.as_mut() {
            draft.answers.extend(results);
        }
    }

    /// Close one tool round-trip inside the current turn: seal the assistant
    /// message that asked (text + its `tool_use` blocks), then the `result`
    /// message that answers it, and reopen the accumulator for the assistant's
    /// continuation.
    ///
    /// Both land in the working view too, so the next request carries the pair —
    /// which is exactly what lets the model continue on the tool's output.
    pub fn finish_tool_round(&mut self, result: Message) {
        let Some(draft) = self.in_progress.as_mut() else {
            return;
        };
        // Count what this round showed before the text is taken, or the turn's
        // running total loses a round every time one finishes.
        draft.shown += shown_of(&draft.assistant, &draft.blocks);
        let asked = Message {
            role: Role::Assistant,
            content: std::mem::take(&mut draft.assistant),
            blocks: std::mem::take(&mut draft.blocks),
        };
        // Straight into the transcript — the turn is the slice, so there is no
        // second copy to keep in step.
        self.messages.push(asked);
        self.messages.push(result);
    }

    /// Does the streaming assistant message carry an unanswered `tool_use`?
    pub fn has_pending_tool_use(&self) -> bool {
        self.pending_tool_uses() > 0
    }

    /// The ids of the `tool_use` blocks the streaming assistant message is
    /// waiting on. Callers match results against these rather than counting:
    /// two tallies that must agree is a bug waiting to happen, and this one
    /// already went wrong once.
    pub fn pending_tool_use_ids(&self) -> Vec<String> {
        self.in_progress
            .as_ref()
            .map(|d| {
                d.blocks
                    .iter()
                    .filter_map(|b| match b {
                        Block::ToolUse { id, .. } => Some(id.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Has this turn completed at least one tool round-trip?
    fn has_finished_rounds(&self) -> bool {
        self.in_progress
            .as_ref()
            .is_some_and(|d| self.messages.len() > d.start + 1)
    }

    /// How many `tool_use` blocks the streaming assistant message is waiting on.
    ///
    /// The API requires a `tool_result` for **every** one; replying with fewer is
    /// a protocol error, so the caller checks this rather than "at least one
    /// result arrived".
    pub fn pending_tool_uses(&self) -> usize {
        // The calls, not every block: a thinking block rides the message too.
        self.pending_tool_use_ids().len()
    }

    /// Seal the in-flight assistant reply into history. Records the
    /// assistant message in the working view **and** seals the
    /// (user, assistant) pair into a content-addressed [`Turn`] appended
    /// to the default episode. Returns the committed assistant text. No-op
    /// returning an empty string if no turn was in progress.
    ///
    /// **Tests only**: it holds an empty table, so its turn records no handle
    /// held, and a production commit that used it would make a session's
    /// secrets unrestorable. Compiled out of the binary, so none can (C-S5,
    /// round 19).
    #[cfg(any(test, feature = "test-util"))]
    pub fn commit_turn(&mut self) -> String {
        self.seal(&crate::Aliases::default())
    }

    /// Seal as `commit_turn` describes, recording in the sealed turn's
    /// provenance the secret handles it names that `held` holds — the only
    /// handles a resumed history asks the vault about (see
    /// `Provenance::handles`). What every production commit calls.
    pub fn commit_turn_holding(&mut self, held: &crate::Aliases) -> String {
        self.seal(held)
    }

    fn seal(&mut self, held: &crate::Aliases) -> String {
        match self.in_progress.take() {
            Some(draft) => {
                // Seal the content-addressed turn for the graph base. The
                // timestamp is provenance (not hashed), so `now()` here does
                // not affect the turn's identity.
                // An UNANSWERED `tool_use` must never be sealed: the API pairs
                // each one with a `tool_result`, so persisting a dangling
                // request means every later request in the episode is rejected —
                // and on resume the poison comes back from disk. Record what
                // actually happened instead: the model asked, and nothing ran.
                // Prefer the answer that actually arrived. Synthesizing over a
                // real result is how a command that ran got recorded as one
                // that did not.
                let answered = |id: &str| {
                    draft.answers.iter().find(
                        |b| matches!(b, Block::ToolResult { tool_use_id, .. } if tool_use_id == id),
                    )
                };
                let unanswered: Vec<Block> = draft
                    .blocks
                    .iter()
                    .filter_map(|b| match b {
                        Block::ToolUse { id, .. } => {
                            Some(answered(id).cloned().unwrap_or(Block::ToolResult {
                                tool_use_id: id.clone(),
                                content: "the turn ended before this ran".to_string(),
                                is_error: true,
                            }))
                        }
                        _ => None,
                    })
                    .collect();
                // Append the final reply (and, if the model's last request went
                // unanswered, the record that nothing ran) to the transcript…
                self.messages.push(Message {
                    role: Role::Assistant,
                    content: draft.assistant.clone(),
                    blocks: draft.blocks,
                });
                if !unanswered.is_empty() {
                    self.messages.push(Message {
                        role: Role::User,
                        content: String::new(),
                        blocks: unanswered,
                    });
                }
                // …then seal exactly that slice. One turn = the prompt, every
                // round-trip it took, and the final reply — and the graph cannot
                // disagree with the transcript, because it IS the transcript.
                let messages = self.messages[draft.start..].to_vec();
                // Sealed live: composed from nothing, so nothing vouches.
                let handles = held.handles_to_record(&messages, &[]);
                let mut turn = Turn::seal(messages, draft.config.clone(), SystemTime::now())
                    .with_handles(handles);
                // Fold in the turn's metadata (neither affects the content-
                // address). Both `take`n so they can't leak into the next turn.
                let cost = self.pending_cost.take();
                turn.provenance.usage = cost.map(|c| c.whole);
                turn.provenance.last_round = cost.map(|c| c.final_round);
                // The history that window measured: this episode's, as it is
                // before the turn joins it.
                turn.provenance.follows = Some(
                    self.graph
                        .full_turns(&self.episode)
                        .last()
                        .map_or(Follows::Start, |&id| Follows::Turn(id)),
                );
                turn.provenance.stop_reason = self.pending_stop.take();
                let occurrence = self.graph.admit(turn);
                if let Some(ep) = self.graph.episode_mut(&self.episode) {
                    ep.append(occurrence);
                }
                draft.assistant
            }
            None => String::new(),
        }
    }

    /// Abandon the in-flight turn after a backend failure: drop the
    /// user message recorded by `begin_turn` so the transcript stays
    /// role-alternating, and discard the partial assistant text. Seals
    /// nothing into the graph (sealing happens only at commit), so the
    /// base needs no cleanup. No-op if no turn was in progress.
    ///
    /// Prefer [`interrupt_turn_holding`](Self::interrupt_turn_holding) on error paths — it
    /// preserves billed partial content; this destroys it. `fail_turn`
    /// remains for the nothing-streamed case (which `interrupt_turn`
    /// delegates here) and for callers that deliberately discard.
    pub fn fail_turn(&mut self) {
        if let Some(draft) = self.in_progress.take() {
            // The turn IS `messages[start..]`, so undoing it is one truncate —
            // no count to compute and get wrong. (Popping a fixed ONE used to
            // strand an unanswered `tool_use`, which the API rejects, and the
            // rejection came back through here and popped again: every later
            // turn failed until `:new`.)
            self.messages.truncate(draft.start);
        }
        self.pending_cost = None;
        self.pending_stop = None;
    }

    /// The stream failed or was cancelled mid-turn: seal the billed PARTIAL
    /// as a real, committed turn marked [`StopReason::Interrupted`] — history
    /// is the truth, and tokens you paid for must not evaporate from the
    /// record. Falls back to [`fail_turn`](Self::fail_turn)'s drop when
    /// nothing streamed yet (an empty assistant message is not a turn, and
    /// providers reject empty content blocks). Returns `true` iff a turn was
    /// committed — the caller persists it and can offer `:continue`.
    ///
    /// **Tests only**, for the reason `commit_turn` is.
    #[cfg(any(test, feature = "test-util"))]
    pub fn interrupt_turn(&mut self) -> bool {
        self.interrupt(&crate::Aliases::default())
    }

    /// Interrupt as `interrupt_turn` describes, recording the held handles as
    /// [`commit_turn_holding`](Self::commit_turn_holding) does. What every
    /// production interrupt calls.
    pub fn interrupt_turn_holding(&mut self, held: &crate::Aliases) -> bool {
        self.interrupt(held)
    }

    fn interrupt(&mut self, held: &crate::Aliases) -> bool {
        match self.in_progress.as_ref() {
            // A turn that RAN COMMANDS has content worth sealing even if the
            // continuation produced no text: the round-trips are the record of
            // what touched the machine, and dropping them made
            // "billed partials are never destroyed" quietly stop applying the
            // moment a tool call happened.
            Some(draft) if !draft.assistant.is_empty() || self.has_finished_rounds() => {
                // Interrupted overrides any stop reason the stream reported
                // earlier — the turn did NOT end the way the API said it
                // would; it ended by failing.
                self.pending_stop = Some(StopReason::Interrupted);
                self.seal(held);
                true
            }
            _ => {
                self.fail_turn();
                false
            }
        }
    }
}

/// **What a round of a reply draws**, in characters: its text and its
/// thinking's (`Output::append_thought`) — an empty or encrypted block draws
/// none of what it was billed.
fn shown_of(text: &str, blocks: &[Block]) -> usize {
    let thought = |block: &Block| match block {
        Block::Thinking { thinking, .. } => thinking.chars().count(),
        _ => 0,
    };
    text.chars().count() + blocks.iter().map(thought).sum::<usize>()
}

impl Default for Conversation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Role;
    use crate::graph::TurnId;

    /// **The default episode's id never moves** — the value stores written
    /// before `:new` keyed their one session by, and a launch's fallback when
    /// a store has no `HEAD`. Derived again, it moved with the tag (11-08),
    /// and such a session would no longer resume: its file kept, unopened.
    #[test]
    fn the_default_episode_id_never_moves() {
        assert_eq!(
            Conversation::default_episode_id().to_hex(),
            "fc7918eb04c93fd6eb19ba2cbf7bb5c8d387be9c8bce63cc4118cb8a04552944"
        );
    }

    // --- What `seal` records (review stop 12, T9 8) ------------------------
    //
    // Each was held only by the TUI's tests, or by none: `cargo test -p
    // shannon-core` saw no regression in any.

    /// **A sealed turn records the history it followed** — the start for a
    /// session's first, the turn before it after. `window::measures` trusts
    /// a measured window only where it follows what is there; sealed always
    /// following the start, every window after the first read as `~`, and
    /// both crates passed.
    #[test]
    fn a_sealed_turn_records_what_it_follows() {
        let mut c = Conversation::new();
        c.begin_turn("first");
        c.push_chunk("one");
        c.commit_turn();
        c.begin_turn("second");
        c.push_chunk("two");
        c.commit_turn();
        let turns = c.own_turns();
        assert_eq!(turns[0].provenance.follows, Some(Follows::Start));
        assert_eq!(
            turns[1].provenance.follows,
            Some(Follows::Turn(turns[0].id))
        );
    }

    /// **A turn of several rounds records the whole bill and the last
    /// round's fill apart** — `whole` is what was billed, `final_round` what
    /// the context window holds; a seal of `whole` for both passed core.
    #[test]
    fn a_turn_records_its_whole_cost_and_its_last_round_apart() {
        let mut c = Conversation::new();
        c.begin_turn("go");
        c.push_chunk("done");
        let whole = crate::Usage {
            input_tokens: 300,
            output_tokens: 40,
            ..Default::default()
        };
        let final_round = crate::Usage {
            input_tokens: 200,
            output_tokens: 10,
            ..Default::default()
        };
        assert_ne!(whole, final_round, "premise: two rounds, two intervals");
        c.record_usage(TurnCost { whole, final_round });
        c.commit_turn();
        let turn = c.own_turns().remove(0);
        assert_eq!(turn.provenance.usage, Some(whole), "the bill");
        assert_eq!(turn.provenance.last_round, Some(final_round), "the fill");
    }

    /// **A tool answer that arrived is sealed as it arrived** — never the
    /// synthesized "the turn ended before this ran" over a command that ran.
    #[test]
    fn a_tool_answer_that_arrived_is_sealed_not_synthesized() {
        let mut c = Conversation::new();
        c.begin_turn("go");
        c.push_tool_use(Block::bash_call("tu_1", "ls"));
        c.keep_tool_answers([Block::ToolResult {
            tool_use_id: "tu_1".into(),
            content: "the real output".into(),
            is_error: false,
        }]);
        c.commit_turn();
        let blocks: Vec<Block> = (c.own_turns().remove(0).messages.into_iter())
            .flat_map(|m| m.blocks)
            .collect();
        let result = blocks.iter().find_map(|b| match b {
            Block::ToolResult { content, .. } => Some(content.as_str()),
            _ => None,
        });
        assert_eq!(result, Some("the real output"), "{blocks:?}");
    }

    /// **A turn is sealed under the config it began under** — a `:model`
    /// between two rounds changes the session's, not the turn's: it ran on
    /// what it began on.
    #[test]
    fn a_turn_is_sealed_under_the_config_it_began_under() {
        let mut c = Conversation::new();
        let began = c.config.model.clone();
        c.begin_turn("go");
        c.set_model("another-model");
        assert_ne!(c.config.model, began, "premise: the session's changed");
        c.push_chunk("done");
        c.commit_turn();
        assert_eq!(c.own_turns().remove(0).provenance.config.model, began);
    }

    #[test]
    fn commit_lifecycle_seals_ordered_content_addressed_turns() {
        // The whole happy-path contract in one walk (consolidated in the M3
        // test review from four bookkeeping tests): chunks accumulate; commit
        // returns the reply, keeps the working view role-alternating, seals a
        // content-addressed turn onto the episode, in order, with the head
        // tracking the latest.
        let mut c = Conversation::new();
        c.begin_turn("hi");
        assert!(c.turn_in_progress());
        c.push_chunk("Hel");
        c.push_chunk("lo.");
        assert_eq!(c.commit_turn(), "Hello.");
        assert!(!c.turn_in_progress());
        c.begin_turn("more");
        c.push_chunk("Sure.");
        c.commit_turn();

        // Working view: role-alternating, both turns.
        let m = c.messages();
        assert_eq!(m.len(), 4);
        assert_eq!((m[0].role, m[0].content.as_str()), (Role::User, "hi"));
        assert_eq!(
            (m[1].role, m[1].content.as_str()),
            (Role::Assistant, "Hello.")
        );
        assert_eq!(m[3].content, "Sure.");

        // Graph base: two sealed turns, in order, genuinely content-addressed,
        // under the active config; head = the latest.
        let ep = c.graph.episode(&c.episode).expect("default episode exists");
        let turns = ep.own_turns();
        assert_eq!(turns.len(), 2);
        let first = c.own_turns().remove(0);
        assert_eq!(
            first.messages,
            vec![Message::user("hi"), Message::assistant("Hello.")],
        );
        assert_eq!(first.provenance.config, c.config, "sealed under our config");
        assert_eq!(
            turns[0],
            TurnId::compute(&first.messages, &c.config),
            "the id IS the content hash"
        );
        assert_eq!(ep.own_head(), Some(turns[1]));
        assert_eq!(
            c.head_turn().expect("head resolves").messages[0].content,
            "more"
        );
    }

    #[test]
    fn fail_drops_user_message_and_seals_nothing() {
        let mut c = Conversation::new();
        c.begin_turn("first");
        c.push_chunk("partial");
        c.fail_turn();
        assert!(!c.turn_in_progress());
        assert!(
            c.messages().is_empty(),
            "failed turn leaves no dangling user message"
        );
        let ep = c.graph.episode(&c.episode).expect("default episode exists");
        assert!(ep.own_is_empty(), "no turn sealed on failure");
        assert_eq!(ep.own_head(), None);

        // A subsequent successful turn behaves normally.
        c.begin_turn("second");
        c.commit_turn();
        assert_eq!(c.messages().len(), 2);
        assert_eq!(c.messages()[0].content, "second");
    }

    #[test]
    fn snapshot_includes_pending_user_message() {
        let mut c = Conversation::new();
        c.begin_turn("ask");
        // The backend needs to see the user message it's replying to.
        let snap = c.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].content, "ask");
    }

    #[test]
    fn push_chunk_without_turn_is_noop() {
        let mut c = Conversation::new();
        c.push_chunk("orphan");
        assert!(c.messages().is_empty());
        assert!(!c.turn_in_progress());
    }

    // --- graph base (the slice-3 façade) ---

    #[test]
    fn resume_rebuilds_the_working_view_and_graph() {
        // Commit two turns, snapshot the episode + its turns, then resume from
        // them into a brand-new conversation (the startup path).
        let mut original = Conversation::new();
        original.begin_turn("one");
        original.push_chunk("1");
        original.commit_turn();
        original.begin_turn("two");
        original.push_chunk("2");
        original.commit_turn();

        let episode = original.episode().clone();
        let turns = original.own_turns();

        let resumed = Conversation::resume(episode, turns);
        // Working view is the flattened transcript, oldest first.
        let m = resumed.messages();
        assert_eq!(m.len(), 4);
        assert_eq!(m[0].content, "one");
        assert_eq!(m[1].content, "1");
        assert_eq!(m[3].content, "2");
        // The graph picked up where it left off: same head, two turns.
        assert_eq!(resumed.episode().own_len(), 2);
        assert_eq!(resumed.head_turn(), original.head_turn());
    }

    #[test]
    fn set_model_seals_new_turns_under_the_model() {
        // `set_model` updates the active config AND the episode's forward config,
        // so a turn committed after it records the real model.
        let mut c = Conversation::new(); // placeholder "unspecified"
        c.set_model("claude-haiku-4-5");
        assert_eq!(c.config.model, "claude-haiku-4-5", "active config");
        assert_eq!(
            c.episode().config.model,
            "claude-haiku-4-5",
            "episode forward config",
        );

        c.begin_turn("u");
        c.push_chunk("a");
        c.commit_turn();
        assert_eq!(
            c.head_turn().unwrap().provenance.config.model,
            "claude-haiku-4-5",
            "the committed turn records the set model",
        );
    }

    #[test]
    fn commit_turn_records_usage_and_clears_it_between_turns() {
        use crate::Usage;
        let mut c = Conversation::new();
        c.begin_turn("hi");
        c.push_chunk("there");
        c.record_usage(TurnCost::single_round(Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        }));
        c.commit_turn();
        assert_eq!(
            c.head_turn().unwrap().provenance.usage,
            Some(Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            }),
            "the turn records the usage it ran under",
        );

        // The next turn, with no usage recorded, doesn't inherit the last one.
        c.begin_turn("again");
        c.push_chunk("ok");
        c.commit_turn();
        assert_eq!(
            c.head_turn().unwrap().provenance.usage,
            None,
            "usage does not leak into the next turn",
        );
    }

    /// Two identical exchanges are two turns, each with its own bill. They are
    /// one CONTENT (one id); keyed by it, the second commit overwrote the
    /// first's usage, so a resumed session under-reported what it had cost.
    #[test]
    fn identical_exchanges_keep_their_own_usage() {
        use crate::Usage;
        let mut c = Conversation::new();
        for output_tokens in [10, 20] {
            c.begin_turn("hi");
            c.push_chunk("Hello.");
            c.record_usage(TurnCost::single_round(Usage {
                output_tokens,
                ..Default::default()
            }));
            c.commit_turn();
        }
        let turns = c.own_turns();
        assert_eq!(turns[0].id, turns[1].id, "premise: one content");
        let billed: Vec<_> = turns
            .iter()
            .map(|t| t.provenance.usage.map(|u| u.output_tokens))
            .collect();
        assert_eq!(billed, vec![Some(10), Some(20)]);
    }

    /// **A missing body stops the read; it never shifts the next turn into
    /// its place.** A persist writes these by position, so returning the
    /// second turn as if it were the first would file it wrong and move the
    /// cursor past both.
    #[test]
    fn own_turns_stop_at_a_missing_body() {
        use crate::graph::{Episode, EpisodeId, TurnConfig};
        let cfg = TurnConfig {
            model: "m".into(),
            max_tokens: None,
            effort: None,
        };
        let seal = |text: &str| {
            Turn::seal(
                vec![Message::user(text), Message::assistant("ok")],
                cfg.clone(),
                std::time::SystemTime::UNIX_EPOCH,
            )
        };
        let (first, second) = (seal("first"), seal("second"));
        let mut episode = Episode::root(EpisodeId::from_seed(b"gap"), cfg.clone());
        episode.append(first.occurrence());
        episode.append(second.occurrence());
        // Resumed without the first turn's body.
        let c = Conversation::resume(episode, vec![second.clone()]);
        assert!(c.own_turns_from(0).is_empty(), "stops at the gap");
        assert_eq!(c.own_turns_from(1), vec![second], "past it, what is there");
    }

    /// A turn that completed tool round-trips appended TWO messages per round;
    /// failing it used to pop exactly one, stranding an unanswered `tool_use` at
    /// the end of the transcript. The API rejects that, the error came back
    /// through this same path, and it popped again — every later turn 400d until
    /// `:new`. (Reviewer A.)
    #[test]
    fn failing_a_turn_removes_every_message_it_appended() {
        let mut c = Conversation::new();
        let before = c.messages().len();
        c.begin_turn("clean up");
        c.push_chunk("Let me look.");
        c.push_tool_use(Block::ToolUse {
            id: "tu_1".into(),
            name: "bash".into(),
            input: serde_json::json!({"command": "ls"}),
        });
        c.finish_tool_round(Message::tool_result("tu_1", "a\nb", false));
        assert_eq!(c.messages().len(), before + 3, "user + asked + answered");

        c.fail_turn();
        assert_eq!(c.messages().len(), before, "the turn leaves nothing behind");
        assert!(
            !c.messages().iter().any(|m| m.tool_uses().next().is_some()),
            "no dangling tool_use survives"
        );
    }

    /// A turn that ran commands is worth sealing even if the continuation
    /// produced no text — the round-trips are the record of what touched the
    /// machine.
    #[test]
    fn interrupting_after_a_tool_round_keeps_the_work() {
        let mut c = Conversation::new();
        c.begin_turn("clean up");
        c.push_tool_use(Block::ToolUse {
            id: "tu_1".into(),
            name: "bash".into(),
            input: serde_json::json!({"command": "ls"}),
        });
        c.finish_tool_round(Message::tool_result("tu_1", "out", false));
        assert!(c.interrupt_turn(), "sealed rather than discarded");
        assert!(c.head_turn().is_some());
    }

    /// **A reply's thinking rides it where it was written, and is no call** —
    /// the calls waiting are the calls (a count of every block read a thought
    /// as one), and the round sealed goes back in the order it came.
    #[test]
    fn thinking_rides_the_reply_where_it_was_written() {
        let mut c = Conversation::new();
        c.begin_turn("look");
        let thought = |s: &str| Block::Thinking {
            thinking: s.into(),
            signature: "S".into(),
            at: 0,
        };
        c.push_thinking(thought("plan"));
        c.push_chunk("Let me look.");
        c.push_thinking(thought("checking"));
        c.push_tool_use(Block::bash_call("tu_1", "ls"));
        assert_eq!(c.pending_tool_uses(), 1, "the call, not the thinking");
        assert_eq!(c.pending_tool_use_ids(), ["tu_1"]);
        c.finish_tool_round(Message::tool_result("tu_1", "a", false));
        let wire = serde_json::to_value(&c.messages()[1]).unwrap();
        let kinds: Vec<&str> = (wire["content"].as_array().unwrap().iter())
            .map(|b| b["type"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["thinking", "text", "thinking", "tool_use"]);
        // And the reply that ends the turn keeps its thinking too.
        c.push_thinking(thought("done"));
        c.push_chunk("Two files.");
        c.commit_turn();
        let sealed = c.head_turn().expect("sealed");
        let last = sealed.messages.last().unwrap();
        assert_eq!(last.blocks.len(), 1, "{last:?}");
        assert_eq!(last.blocks[0].at(), Some(0), "before its text");
    }

    /// **What a turn showed counts its thinking**, in every round: the
    /// transcript draws it, so a turn that mostly thought is not "billed for
    /// nothing shown" (N1b). An empty block draws nothing, and counts none.
    #[test]
    fn what_a_turn_showed_counts_its_thinking() {
        let mut c = Conversation::new();
        c.begin_turn("look");
        let thought = |s: &str| Block::Thinking {
            thinking: s.into(),
            signature: "S".into(),
            at: 0,
        };
        c.push_thinking(thought("plan"));
        c.push_chunk("ab");
        c.push_tool_use(Block::bash_call("tu_1", "ls"));
        assert_eq!(c.draft_len(), Some(6), "this round: text and thinking");
        c.finish_tool_round(Message::tool_result("tu_1", "a", false));
        c.push_thinking(thought(""));
        c.push_thinking(thought("ü"));
        assert_eq!(c.draft_len(), Some(7), "a finished round kept; chars");
    }

    /// An UNANSWERED `tool_use` must never be sealed: persisted, it poisons
    /// every later request in the episode and comes back from disk on resume.
    /// It is recorded as "asked, nothing ran" instead — and the graph and the
    /// working view must agree, or the on-disk copy stays broken while the
    /// in-session one looks fine.
    #[test]
    fn committing_never_seals_an_unanswered_tool_use() {
        let mut c = Conversation::new();
        c.begin_turn("clean up");
        c.push_chunk("Let me check.");
        c.push_tool_use(Block::ToolUse {
            id: "tu_1".into(),
            name: "bash".into(),
            input: serde_json::json!({"command": "rm /tmp/a"}),
        });
        c.commit_turn();

        let sealed = c.head_turn().expect("sealed").messages.clone();
        let uses: Vec<_> = sealed.iter().flat_map(|m| m.tool_uses()).collect();
        assert_eq!(uses.len(), 1, "the request is still recorded");
        let answered: Vec<_> = sealed
            .iter()
            .flat_map(|m| m.blocks.iter())
            .filter_map(|b| match b {
                Block::ToolResult {
                    tool_use_id,
                    is_error,
                    ..
                } => Some((tool_use_id, *is_error)),
                _ => None,
            })
            .collect();
        assert_eq!(answered.len(), 1, "and it is answered: {sealed:#?}");
        assert_eq!(answered[0].0, "tu_1");
        assert!(answered[0].1, "answered as an error — nothing ran");

        // The working view must carry exactly what was sealed.
        let view_blocks: Vec<_> = c.messages().iter().flat_map(|m| m.blocks.iter()).collect();
        assert_eq!(
            view_blocks.len(),
            2,
            "graph and transcript agree: {:#?}",
            c.messages()
        );
    }

    #[test]
    fn commit_turn_records_the_stop_reason_and_clears_it() {
        use crate::StopReason;
        let mut c = Conversation::new();
        c.begin_turn("hi");
        c.push_chunk("cut off mid-");
        c.record_stop_reason(StopReason::MaxTokens);
        c.commit_turn();
        assert_eq!(
            c.head_turn().unwrap().provenance.stop_reason,
            Some(StopReason::MaxTokens),
            "the turn records why it ended — truncation is visible",
        );

        // The next turn doesn't inherit the last one's reason.
        c.begin_turn("again");
        c.push_chunk("ok");
        c.commit_turn();
        assert_eq!(c.head_turn().unwrap().provenance.stop_reason, None);
    }

    /// **Every seal records what it held, and the plain doors hold nothing.**
    /// The `_holding` doors record the handles the turn names that the table
    /// holds; the plain ones record `Some(vec![])` — fail-closed — never
    /// `None`, which means only "read from a log older than the field" and is
    /// what recall's legacy scan keys on.
    #[test]
    fn every_door_records_the_handles_it_held() {
        let mut held = crate::Aliases::default();
        let handle = held
            .insert("pw", "hunter2", crate::MARKED_SOURCE, true)
            .unwrap();
        let handles = |c: &Conversation| c.head_turn().unwrap().provenance.handles;
        let mut c = Conversation::new();

        c.begin_turn(format!("use {handle}"));
        c.push_chunk("ok");
        c.commit_turn_holding(&held);
        assert_eq!(handles(&c), Some(vec![handle.clone()]), "commit, holding");

        c.begin_turn(format!("use {handle}"));
        c.push_chunk("ok");
        c.commit_turn();
        assert_eq!(handles(&c), Some(vec![]), "commit, plain: none held");

        c.begin_turn(format!("use {handle}"));
        c.push_chunk("partial");
        assert!(c.interrupt_turn_holding(&held));
        assert_eq!(
            handles(&c),
            Some(vec![handle.clone()]),
            "interrupt, holding"
        );

        c.begin_turn(format!("use {handle}"));
        c.push_chunk("partial");
        assert!(c.interrupt_turn());
        assert_eq!(handles(&c), Some(vec![]), "interrupt, plain: none held");
    }

    #[test]
    fn interrupt_seals_the_billed_partial_as_a_real_turn() {
        use crate::{StopReason, Usage};
        let mut c = Conversation::new();
        c.begin_turn("first");
        c.push_chunk("partial answ");
        c.record_usage(TurnCost::single_round(Usage {
            input_tokens: 10,
            output_tokens: 3,
            ..Default::default()
        }));
        assert!(c.interrupt_turn(), "a partial with content commits");
        assert!(!c.turn_in_progress());

        // History is the truth: the partial is a real turn, marked and priced.
        let head = c.head_turn().expect("the partial was sealed");
        assert_eq!(head.messages[1].content, "partial answ");
        assert_eq!(
            head.provenance.stop_reason,
            Some(StopReason::Interrupted),
            "Interrupted overrides — the turn ended by failing",
        );
        assert_eq!(
            head.provenance.usage.map(|u| u.output_tokens),
            Some(3),
            "what it cost so far is recorded",
        );
        // The working view stays role-alternating with the partial in place.
        assert_eq!(c.messages().len(), 2);
        assert_eq!(c.messages()[1].content, "partial answ");
    }

    #[test]
    fn interrupt_with_nothing_streamed_falls_back_to_the_drop() {
        use crate::Usage;
        let mut c = Conversation::new();
        c.begin_turn("first");
        c.record_usage(TurnCost::single_round(Usage {
            input_tokens: 10,
            output_tokens: 0,
            ..Default::default()
        }));
        assert!(!c.interrupt_turn(), "no assistant content → nothing sealed");
        assert!(c.messages().is_empty(), "the user message is dropped");
        assert!(c.episode().own_is_empty(), "nothing sealed into the graph");

        // The stale usage must not leak into the NEXT committed turn.
        c.begin_turn("second");
        c.push_chunk("ok");
        c.commit_turn();
        assert_eq!(
            c.head_turn().unwrap().provenance.usage,
            None,
            "a failed turn's pending metadata is cleared",
        );
    }
}
