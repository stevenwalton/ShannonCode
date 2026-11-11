//! The in-memory **Graph**: every [`Episode`] and [`Turn`] keyed by id, plus the
//! one recursive [`Node`] abstraction that lets a single traversal (and a single
//! viewer, in P4) walk every level of the fractal structure uniformly.
//!
//! The structure is recursive (design.md §1; Milestone 3 "the fractal structure
//! to get right"):
//!
//! ```text
//! Graph → Episode → Turn → message
//!                     └─ a tool-call may root a child Episode (sub-agent),
//!                        which is itself an Episode, recursively
//! ```
//!
//! The bet of M3 is that *one* node type expresses all of these so nothing
//! special-cases a level. [`Node`] is that type; [`Graph::children`] is the one
//! step a traversal repeats. This is the **read side** only — it's built from
//! inserted episodes/turns and queried; the write-side op-set (`branch`,
//! `clone`, `squash`, …) lands in the ops phase (P5).

use std::collections::HashMap;

use tracing::warn;

use crate::graph::episode::{EdgeKind, Episode, EpisodeId, Origin, Segment, push_own};
use crate::graph::turn::{Occurrence, Provenance, Turn, TurnConfig, TurnId};
use crate::message::{Block, Message, Role};

/// One node in the recursive/fractal structure — an Episode, a Turn within an
/// episode, or a message within that turn. The single abstraction the traversal
/// and the viewer share across every level.
///
/// Turn and Message nodes are **qualified by their `episode`** on purpose: with
/// content-addressing the same [`TurnId`] can recur across episodes, so a turn's
/// place in the graph is only well-defined together with the episode it sits in.
/// (The `tool-call` sub-agent variant rides the existing Episode node reached
/// through a turn — no new variant needed; it's earned in P6.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Node {
    /// A whole episode.
    Episode(EpisodeId),
    /// A turn at a position within an episode.
    Turn {
        episode: EpisodeId,
        turn: TurnId,
        /// The turn's position in `episode`'s full derived sequence — its
        /// **occurrence** identity. Content-addressed ids recur, so `turn` alone
        /// can't tell two identical turns apart; the index does, and every
        /// selection→edit/checkout consumes it directly instead of re-searching by
        /// content (the [[content-addressed-ids]] trap).
        index: usize,
    },
    /// One message inside a turn.
    Message {
        episode: EpisodeId,
        turn: TurnId,
        /// The parent turn's occurrence index in `episode`'s full sequence (carried
        /// down from the [`Turn`](Self::Turn) node), so a message-level
        /// branch/checkout targets the right occurrence of a repeated turn.
        turn_index: usize,
        /// This message's index within the turn.
        index: usize,
    },
}

/// One position of an episode's derived sequence, **placed**: the turn, the
/// episode that owns it (holds its [`Occurrence`] in a [`Segment::Own`]), its
/// position in that owner's derived sequence, and the provenance of that one
/// occurrence.
///
/// `(owner, owner_index)` is the occurrence's identity — a durable
/// back-reference, since the append-only invariant keeps an existing turn's
/// derived index stable. It is what lets [`Graph::project`] emit a slice
/// pointing straight at the data owner, and what makes two identical turns two
/// things: one `turn`, two placements, each with its own facts.
#[derive(Debug, Clone, Copy)]
pub struct Placed<'g> {
    /// The content's address.
    pub turn: TurnId,
    /// The episode whose `Own` segment holds this occurrence.
    pub owner: EpisodeId,
    /// This occurrence's position in `owner`'s derived sequence.
    pub owner_index: usize,
    /// The facts of this production — not of the content's other occurrences.
    pub provenance: &'g Provenance,
}

impl Placed<'_> {
    /// Whether two placements are ONE occurrence — shared history, as opposed
    /// to two productions that happen to have the same content.
    pub fn same_occurrence(&self, other: &Placed<'_>) -> bool {
        self.owner == other.owner && self.owner_index == other.owner_index
    }
}

/// The in-memory conversation graph: episodes by id, and turn content by its
/// address. The persistence layer (`Store`) hydrates one of these; the viewer
/// renders it. Reads here are cheap map lookups — never on the render hot path
/// (`[[feedback_perf_target]]`): the TUI walks a loaded graph, it does not call a
/// `Store` per frame.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    episodes: HashMap<EpisodeId, Episode>,
    /// What each turn IS, by content address — shared by every occurrence.
    /// The facts of each production are not here; they are [`Occurrence`]s in
    /// the episodes that produced them (see the `turn` module docs).
    contents: HashMap<TurnId, Content>,
}

/// A turn's content: exactly what its [`TurnId`] hashes — the messages and the
/// model — so it is the same for every occurrence of that id.
///
/// **The body is loaded on demand.** A store may hand over only the
/// [`outline`](MessageOutline) — enough to list, label and inspect a turn —
/// and the messages when something reads them ([`Graph::fill_bodies`]); a
/// launch then costs the graph's structure, not its history (storage.md §4).
#[derive(Debug, Clone)]
struct Content {
    model: String,
    outline: Vec<MessageOutline>,
    /// `None` until loaded.
    messages: Option<Vec<Message>>,
}

/// One message of a turn as a row shows it without the body: who said it,
/// its first line (at most [`OUTLINE_CHARS`] characters) and its length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageOutline {
    pub role: Role,
    /// The first line with any text on it, trimmed — what a label shows. A
    /// message that opens with a blank line still has one.
    pub first_line: String,
    /// Whether [`first_line`](Self::first_line) is someone's words — false
    /// for a message of no words, whose line names the calls or results it
    /// holds instead (`[Bash: rg …]`), so a row need not call a tool's output
    /// the user's.
    pub words: bool,
    /// The whole message's length, in characters.
    pub chars: usize,
}

/// How much of a message's first line an outline keeps — more than any row
/// shows, so a view can truncate to its own width.
pub const OUTLINE_CHARS: usize = 120;

/// The outline of `messages` — what a store keeps beside the structure so the
/// structure alone can draw a turn.
pub fn outline_of(messages: &[Message]) -> Vec<MessageOutline> {
    messages
        .iter()
        .map(|m| {
            let (line, words) = first_line(m);
            MessageOutline {
                role: m.role,
                first_line: line.chars().take(OUTLINE_CHARS).collect(),
                words,
                chars: m.content.chars().count(),
            }
        })
        .collect()
}

/// The first line of `text` with anything on it, trimmed.
fn line_of(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|l| !l.is_empty())
}

/// **What a row says a message is** — its first line of words, from its
/// content or a text block; else, for a message that is all calls or all
/// results, what it holds, in brackets, as the transcript writes what is no
/// one's words (`[an image, not imported]`). A turn's tool rounds were rows
/// reading `asst:` and `user:` over nothing (Steven, 10-16). The flag says
/// which: words (an empty message counts, its row its speaker's), or held.
fn first_line(m: &Message) -> (String, bool) {
    let texts = m.blocks.iter().filter_map(|b| match b {
        Block::Text { text } => Some(text.as_str()),
        _ => None,
    });
    if let Some(words) = std::iter::once(m.content.as_str())
        .chain(texts)
        .find_map(line_of)
    {
        return (words.to_string(), true);
    }
    let parts: Vec<String> = m.blocks.iter().filter_map(described).collect();
    match parts.as_slice() {
        [] => (String::new(), true),
        [one] => (format!("[{one}]"), false),
        [first, rest @ ..] => (format!("[{first}, and {} more]", rest.len()), false),
    }
}

/// A block that is not words, as a row names it: a call by its tool and an
/// argument — a shell call's command; else the first string argument by
/// NAME, as a JSON object holds them sorted (`Grep` shows its `path`, not
/// its `pattern`) — and a result by its first line.
fn described(block: &Block) -> Option<String> {
    match block {
        Block::ToolUse { name, input, .. } => {
            let argument = block.bash_command().or_else(|| {
                input
                    .as_object()?
                    .values()
                    .find_map(|v| v.as_str().and_then(line_of))
            });
            Some(match argument.and_then(line_of) {
                Some(argument) => format!("{name}: {argument}"),
                None => name.clone(),
            })
        }
        Block::ToolResult {
            content, is_error, ..
        } => {
            let what = if *is_error { "error" } else { "result" };
            Some(format!("{what}: {}", line_of(content).unwrap_or("empty")))
        }
        // Thinking is not what a row names: the call it led to, or the reply,
        // says what the message did.
        Block::Text { .. }
        | Block::Context { .. }
        | Block::Thinking { .. }
        | Block::RedactedThinking { .. } => None,
    }
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    /// **Admit a sealed turn**: its content joins the shared set and its
    /// occurrence comes back for the caller to place in a segment.
    ///
    /// The door from a [`Turn`] record into the graph. A record cannot simply
    /// be stored by id: the id names content, so the next identical turn would
    /// overwrite this one's provenance — which is what happened before content
    /// and occurrence were split.
    pub fn admit(&mut self, turn: Turn) -> Occurrence {
        let occurrence = turn.occurrence();
        self.insert_content(turn);
        occurrence
    }

    /// Keep a record's CONTENT — its messages and model — for a loader or
    /// overlay that places the occurrences itself; the record's other facts are
    /// not kept here (they belong to its occurrence). One id is one content, so
    /// a second insert only supplies a body that was not loaded yet.
    pub fn insert_content(&mut self, turn: Turn) {
        match self.contents.get_mut(&turn.id) {
            Some(content) => {
                content.messages.get_or_insert(turn.messages);
            }
            None => {
                let content = Content {
                    model: turn.provenance.config.model,
                    outline: outline_of(&turn.messages),
                    messages: Some(turn.messages),
                };
                self.contents.insert(turn.id, content);
            }
        }
    }

    /// Know a turn by its outline alone — what a store's structure load
    /// supplies. The body comes later, through [`fill_bodies`](Self::fill_bodies).
    /// A turn already known is left as it is.
    pub fn insert_outline(&mut self, id: TurnId, model: String, outline: Vec<MessageOutline>) {
        self.contents.entry(id).or_insert(Content {
            model,
            outline,
            messages: None,
        });
    }

    /// Supply loaded bodies. A body for a turn the graph does not know is
    /// dropped: the outline is what makes it a turn here.
    pub fn fill_bodies(&mut self, bodies: impl IntoIterator<Item = (TurnId, Vec<Message>)>) {
        for (id, messages) in bodies {
            if let Some(content) = self.contents.get_mut(&id) {
                content.messages.get_or_insert(messages);
            }
        }
    }

    /// Of `ids`, the turns known here whose body is not loaded — what a
    /// reader must fetch before it reads them. Each once, in order.
    pub fn unloaded(&self, ids: impl IntoIterator<Item = TurnId>) -> Vec<TurnId> {
        // A set beside the list, not `out.contains`: a hydrate walks a whole
        // history, and a linear search per turn made it quadratic in it.
        let mut seen = std::collections::HashSet::new();
        ids.into_iter()
            .filter(|id| self.contents.get(id).is_some_and(|c| c.messages.is_none()))
            .filter(|id| seen.insert(*id))
            .collect()
    }

    /// Drop every loaded body, keeping the outlines — the graph a
    /// structure-only load gives. For [`LazyBodies`](crate::store::LazyBodies),
    /// which makes a full-loading store answer like a lazy one.
    #[cfg(any(test, feature = "test-util"))]
    pub fn evict_bodies(&mut self) {
        for content in self.contents.values_mut() {
            content.messages = None;
        }
    }

    /// Record (or replace) an episode by its id.
    pub fn insert_episode(&mut self, episode: Episode) {
        self.episodes.insert(episode.id, episode);
    }

    /// A turn's messages — its content, the same for every occurrence. `None`
    /// for a turn the graph does not know **or whose body is not loaded**: a
    /// reader that needs the words fetches them first (`hydrate`); one that
    /// only shows a turn reads its [`outline`](Self::outline).
    pub fn messages(&self, id: &TurnId) -> Option<&[Message]> {
        self.contents.get(id)?.messages.as_deref()
    }

    /// A turn's outline — role, first line and length per message — which a
    /// structure-only load carries. Enough to list and label it.
    pub fn outline(&self, id: &TurnId) -> Option<&[MessageOutline]> {
        self.contents.get(id).map(|c| c.outline.as_slice())
    }

    /// The model that produced a turn. Part of its content address, so the same
    /// for every occurrence — unlike the token cap, which is not hashed and is
    /// read from an occurrence's provenance.
    pub fn model(&self, id: &TurnId) -> Option<&str> {
        self.contents.get(id).map(|c| c.model.as_str())
    }

    /// An episode's derived sequence with every position [`Placed`] — the
    /// occurrence-aware face of [`full_turns`](Self::full_turns), for whatever
    /// reads a production's facts. Derived at query time, never per frame.
    pub fn placed(&self, episode: &EpisodeId) -> Vec<Placed<'_>> {
        self.resolve_annotated(episode, &mut Vec::new())
    }

    /// The record at one placement: the shared content with THIS occurrence's
    /// provenance. `None` when the content is not in the graph or its body is
    /// not loaded.
    pub fn record(&self, placed: &Placed<'_>) -> Option<Turn> {
        let messages = self.messages(&placed.turn)?.to_vec();
        Some(Turn {
            id: placed.turn,
            messages,
            provenance: placed.provenance.clone(),
        })
    }

    /// An episode's derived sequence as records, oldest first — what a working
    /// view replays and a resume renders, each turn carrying the facts of its
    /// own occurrence.
    ///
    /// **`None` unless every body is loaded.** Skipping an unloaded turn would
    /// hand a resume a shorter session than the one on disk, which reads as
    /// history lost; a caller loads the bodies first (`hydrate`).
    pub fn records(&self, episode: &EpisodeId) -> Option<Vec<Turn>> {
        self.placed(episode)
            .iter()
            .map(|p| self.record(p))
            .collect()
    }

    /// The secret handles **any occurrence** of these turns recorded — what
    /// each production held when it was sealed — sorted, each once. What a
    /// composition's sources vouch for: a turn composed from them names their
    /// handles, and in a later run this one's table may not hold them yet, so
    /// recording only what is held now recorded nothing and the secret never
    /// came back (Reviewer A, round 19). Every occurrence, because the same
    /// content sealed in two runs can record differently, and either record
    /// was a real hold. A legacy occurrence (`None`) vouches for nothing.
    pub fn recorded_handles(&self, turns: &[TurnId]) -> Vec<String> {
        let mut out: Vec<String> = self
            .episodes
            .values()
            .flat_map(|e| e.own_occurrences())
            .filter(|o| turns.contains(&o.turn))
            .flat_map(|o| o.provenance.handles.iter().flatten().cloned())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The episode with this id, if recorded.
    pub fn episode(&self, id: &EpisodeId) -> Option<&Episode> {
        self.episodes.get(id)
    }

    /// Every episode id recorded (unordered). Lets a caller ask cross-episode
    /// questions — e.g. "which sessions share this content-addressed turn?".
    pub fn episode_ids(&self) -> Vec<EpisodeId> {
        self.episodes.keys().copied().collect()
    }

    /// The episode's depth in the **ancestry tree** — the length of its origin
    /// chain (a root is 0). This walks spawn relations (`Origin` edges) only,
    /// never the content-sharing (`Slice`) DAG: the two are different graphs,
    /// and guardrails (max spawn depth) are a property of ancestry, not of how
    /// much content happens to be shared. Cycle-guarded like `resolve` — a
    /// corrupt origin loop returns the depth walked so far instead of hanging.
    pub fn depth(&self, episode: &EpisodeId) -> usize {
        self.origin_chain(episode).len()
    }

    /// The episode's **spawn depth** — how many `ToolCall` (child-session)
    /// edges sit along its origin chain. This is the `[agents] max_depth`
    /// guardrail metric, and it deliberately differs from [`depth`](Self::depth):
    /// branches/clones/merges are deliberate user surgery, so a human three
    /// manual branches deep still spawns freely — only recursion (children
    /// spawning children) accumulates.
    pub fn child_depth(&self, episode: &EpisodeId) -> usize {
        self.origin_chain(episode)
            .iter()
            .filter(|o| o.kind == EdgeKind::ToolCall)
            .count()
    }

    /// The origin chain from `episode` up to its root, nearest ancestor first.
    /// Cycle-guarded like `resolve` — a corrupt origin loop yields the chain
    /// walked so far instead of hanging. The shared walk beneath
    /// [`depth`](Self::depth), [`child_depth`](Self::child_depth) and the
    /// TUI's "which session did these turns come in through" (a third walk
    /// there kept its own bound, and nothing pinned it — review stop 5, B 1,
    /// C 4).
    pub fn origin_chain(&self, episode: &EpisodeId) -> Vec<Origin> {
        let mut seen = vec![*episode];
        let mut current = *episode;
        let mut chain = Vec::new();
        while let Some(origin) = self.episodes.get(&current).and_then(|e| e.origin) {
            if seen.contains(&origin.episode) {
                warn!(episode = %origin.episode.to_hex(), "origin cycle detected; chain truncated");
                break;
            }
            seen.push(origin.episode);
            current = origin.episode;
            chain.push(origin);
        }
        chain
    }

    /// The episodes whose origin points at `episode` — its **spawn children** in
    /// the ancestry tree (any [`EdgeKind`]), sorted by id for determinism. The
    /// per-turn fan-out is the same query narrowed to one attach turn, which
    /// [`children`](Self::children) already answers.
    pub fn spawn_children(&self, episode: &EpisodeId) -> Vec<EpisodeId> {
        let mut kids: Vec<EpisodeId> = self
            .episodes
            .values()
            .filter(|e| e.origin.is_some_and(|o| o.episode == *episode))
            .map(|e| e.id)
            .collect();
        kids.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        kids
    }

    /// The episode's **child sessions** — [`spawn_children`](Self::spawn_children)
    /// narrowed to `ToolCall` edges, sorted by id. The `[agents] max_fanout`
    /// guardrail counts these: a branch or clone is user surgery, not fan-out,
    /// so a session with many manual branches still spawns freely.
    pub fn child_sessions(&self, episode: &EpisodeId) -> Vec<EpisodeId> {
        self.spawn_children(episode)
            .into_iter()
            .filter(|id| {
                self.episodes
                    .get(id)
                    .and_then(|e| e.origin)
                    .is_some_and(|o| o.kind == EdgeKind::ToolCall)
            })
            .collect()
    }

    /// How many episodes contain `turn` in their **full** history — a
    /// content-addressed turn can recur across sessions/branches (identical turns
    /// dedup to one id), and a turn in a shared prefix belongs to every descendant
    /// too. `> 1` means "shared", which the inspector surfaces so the recurrence
    /// isn't surprising. Derives each episode's full sequence (a shared prefix is
    /// not stored per episode); an inspector query, not a hot path.
    pub fn episodes_with_turn(&self, turn: &TurnId) -> usize {
        self.episodes
            .keys()
            .filter(|id| self.full_turns(id).contains(turn))
            .count()
    }

    /// An episode's **full** turn sequence, oldest first — the logical "ordered
    /// sequence of Turns" (design.md §1.1) materialized by resolving its
    /// [`Segment`]s: `Own` pieces contribute their turns; a `Slice` contributes a
    /// range of the referenced episode's *derived* sequence (shared by reference,
    /// never copied). This is what makes "share unless changed" hold in every
    /// direction — a branch's prefix `Slice`, a re-root's suffix `Slice`, a squash's
    /// flanking `Slice`s all resolve the same way. An unknown id yields an empty
    /// sequence.
    ///
    /// Derived at submit-/query-time, never per frame (`[[feedback_perf_target]]`).
    pub fn full_turns(&self, episode: &EpisodeId) -> Vec<TurnId> {
        self.resolve(episode, &mut Vec::new())
    }

    /// Resolve an episode's segments into its full sequence. Thin projection of
    /// [`resolve_annotated`](Self::resolve_annotated) onto just the turn ids —
    /// one segment-walk skeleton, so derivation and provenance can never drift.
    fn resolve(&self, episode: &EpisodeId, stack: &mut Vec<EpisodeId>) -> Vec<TurnId> {
        self.resolve_annotated(episode, stack)
            .into_iter()
            .map(|p| p.turn)
            .collect()
    }

    /// Resolve an episode's segments into its full sequence, each position
    /// [`Placed`] — who owns it, and the facts of that occurrence. `stack` holds
    /// the episodes currently on the resolution path, so a corrupt *cyclic* slice
    /// reference breaks instead of recursing forever. A legitimate slice DAG has
    /// diamonds (the same episode sliced from two places — e.g. a squash slicing
    /// its parent twice), so the guard is the **stack**, not a global visited set.
    ///
    /// The owner tag is what makes [`project`](Self::project) able to emit a `Slice`
    /// pointing straight at the data-owning episode: an `Own` turn is owned by *this*
    /// episode at its position in this episode's derived sequence; a `Slice`'s turns
    /// **inherit** their owner from the recursion (which has already bottomed out at
    /// the `Own` that holds them), so a projected slice never references a
    /// pass-through intermediate.
    fn resolve_annotated(
        &self,
        episode: &EpisodeId,
        stack: &mut Vec<EpisodeId>,
    ) -> Vec<Placed<'_>> {
        let Some(ep) = self.episodes.get(episode) else {
            // A non-empty stack means we're resolving a *slice source* that's gone —
            // orphaned/corrupt history (e.g. a shared episode deleted out from under a
            // clone). Surface it: in a "history is the truth" store, silently dropping
            // shared turns is worse than a loud warning. (An empty stack is just an
            // unknown top-level id — benign, e.g. a partially hydrated graph.)
            if !stack.is_empty() {
                warn!(episode = %episode.to_hex(), "resolve: slice references a missing episode — orphaned history, resolving empty");
            }
            return Vec::new();
        };
        if stack.contains(episode) {
            warn!(episode = %episode.to_hex(), "resolve: cyclic slice reference — corrupt data, resolving empty");
            return Vec::new(); // cycle — corrupt data, don't loop
        }
        stack.push(*episode);
        let mut seq: Vec<Placed<'_>> = Vec::new();
        for seg in ep.segments() {
            match seg {
                Segment::Own(ts) => {
                    for occurrence in ts {
                        // An own turn is owned by this episode; its owner-index is its
                        // position in this episode's derived sequence (== the length
                        // built so far, since we resolve in order). The append-only
                        // invariant keeps that index stable, so it's a durable ref.
                        let owner_index = seq.len();
                        seq.push(Placed {
                            turn: occurrence.turn,
                            owner: *episode,
                            owner_index,
                            provenance: &occurrence.provenance,
                        });
                    }
                }
                Segment::Slice {
                    episode: src,
                    start,
                    end,
                } => {
                    let resolved = self.resolve_annotated(src, stack);
                    // Clamp against the resolved length — defends against a corrupt or
                    // stale range without panicking. A range that *exceeds* the source
                    // means shared turns are being dropped: surface it rather than
                    // truncate in silence.
                    if *end > resolved.len() {
                        warn!(source = %src.to_hex(), start = *start, end = *end, available = resolved.len(),
                            "resolve: slice range exceeds its source — stale/corrupt, some shared turns dropped");
                    }
                    let s = (*start).min(resolved.len());
                    let e = (*end).min(resolved.len());
                    if s < e {
                        // Carry each turn's TRUE owner through, not `src`.
                        seq.extend_from_slice(&resolved[s..e]);
                    }
                }
            }
        }
        stack.pop();
        seq
    }

    /// **Remove turns from an episode in place**, by position in its derived
    /// sequence, keeping the episode's own id.
    ///
    /// Deleting a turn is a piece-table edit like every other graph surgery, but
    /// it is the one that cannot go through [`compose`](Self::compose): compose
    /// lowers its spans with [`project`](Self::project), which names the *data
    /// owner*, and for an episode's own turns that owner is the episode itself.
    /// Feeding that back in would leave the episode slicing itself — a cycle
    /// `resolve` breaks on, emptying the session it was asked to edit. So the
    /// rebuild keeps the ownership distinction that `project` deliberately
    /// erases: a position this episode OWNS stays an [`Segment::Own`] entry, a
    /// position it borrows stays a [`Segment::Slice`] at the same owner index.
    /// A kept own position keeps its whole [`Occurrence`], provenance included.
    ///
    /// `drop` holds positions in the **derived** sequence — occurrences, not
    /// ids. A content hash recurs, so the same `TurnId` may sit at several
    /// positions and only the index distinguishes them
    /// (`[[content-addressed-ids]]`). Out-of-range positions are ignored;
    /// an unknown episode is a no-op returning `false`.
    ///
    /// **Unguarded surgery, like [`splice`](Self::splice).** Another episode
    /// holding a `Slice` into this one addresses it by *index*, so removing a
    /// position shifts everything after it and that slice silently resolves to
    /// different turns. Core stays total and the caller owns the policy — the
    /// TUI [`detach`](Self::detach)es such a reader first.
    pub fn remove_turns(&mut self, episode: &EpisodeId, drop: &[usize]) -> bool {
        self.reshape(episode, drop, None)
    }

    /// **Make `episode` stop reading from `from`**: every turn it borrows from
    /// `from` becomes its own — the same occurrence, provenance and all —
    /// and every other position stays as it was. Its history does not change
    /// by a turn; only where the data lives does.
    ///
    /// What lets `from` be deleted, or have turns cut, without `episode`
    /// losing or shifting one: a fork keeps its history when the session it
    /// forked from goes, as a git branch keeps commits another branch
    /// deleted. The cost is the copied turns, written into `episode`'s log.
    /// `false` for an unknown episode.
    pub fn detach(&mut self, episode: &EpisodeId, from: &EpisodeId) -> bool {
        self.reshape(episode, &[], Some(from))
    }

    /// Rebuild `episode` from its placements, dropping the positions in
    /// `drop`, with turns owned by itself — and by `adopt`, if given — as
    /// `Own` and every other turn a `Slice` of its owner. The one rebuild
    /// [`remove_turns`](Self::remove_turns) and [`detach`](Self::detach) share.
    fn reshape(&mut self, episode: &EpisodeId, drop: &[usize], adopt: Option<&EpisodeId>) -> bool {
        let Some(ep) = self.episodes.get(episode).cloned() else {
            return false;
        };
        // Where it parted from Claude Code: the imported turns lead it.
        let parted = ep.inherited.as_ref().map_or(0, |i| i.turns);
        let mut dropped_before_parting = 0;
        let placed = self.resolve_annotated(episode, &mut Vec::new());
        let mut segments: Vec<Segment> = Vec::new();
        for (position, p) in placed.iter().enumerate() {
            if drop.contains(&position) {
                dropped_before_parting += usize::from(position < parted);
                continue;
            }
            if p.owner == *episode || Some(&p.owner) == adopt {
                // Our own data. It has to stay `Own` — see the cycle above.
                let occurrence = Occurrence {
                    turn: p.turn,
                    provenance: p.provenance.clone(),
                };
                push_own(&mut segments, occurrence);
            } else if let Some(Segment::Slice {
                episode: owner,
                end,
                ..
            }) = segments.last_mut()
                && *owner == p.owner
                && *end == p.owner_index
            {
                // Same owner, next owner-index: extend rather than open a second
                // slice, so removing nothing rebuilds the same shape.
                *end = p.owner_index + 1;
            } else {
                segments.push(Segment::Slice {
                    episode: p.owner,
                    start: p.owner_index,
                    end: p.owner_index + 1,
                });
            }
        }
        // The same session, reshaped: its name and import record are its
        // own, not its shape's — `splice` minted them afresh (A F1). Each
        // position counts once, however often `drop` lists it, so the
        // parting point cannot pass zero.
        //
        // **An imported turn gone, the turn it parted at is no longer
        // turn-exact**: its imported turns are no longer the run of the
        // conversation's that `:graph` maps them onto — the last one gone, it
        // would hang at a turn it no longer holds; one before it, its
        // branches one turn late (review stop 3, A F7). No place is said;
        // it hangs from the conversation's row, every turn its own.
        let mut rebuilt = ep.with_segments(segments);
        if let Some(inherited) = &mut rebuilt.inherited {
            inherited.turns -= dropped_before_parting;
            if dropped_before_parting > 0 {
                // Each turn's place with the parting's: no record, not one a
                // turn short of the turns it names.
                inherited.at = None;
                inherited.opened.clear();
            }
        }
        self.insert_episode(rebuilt);
        true
    }

    /// The piece-table **substring** (design.md §1.2): the minimal [`Segment`] list
    /// denoting positions `[start, end)` of `episode`'s *derived* sequence, where
    /// every emitted `Slice` references the episode that actually **owns** those
    /// turns — never a pass-through intermediate — with adjacent same-owner runs
    /// coalesced into one slice.
    ///
    /// This is the primitive every mutation composes from, so "share the data unless
    /// it is changed" holds *by construction* at any point, forward or backward:
    ///
    /// ```text
    /// branch    = project(0, k)               ++ Own(edited)
    /// clone     = project(0, len)
    /// re_root   = Own(new_root)               ++ project(from, len)
    /// edit (COW)= project(0, i) ++ Own(edited) ++ project(i+1, len)
    /// squash    = project(0, s) ++ Own(sum)    ++ project(e, len)
    /// merge     = project(each side around the LCA) ++ Own(summary)
    /// ```
    ///
    /// Because the owner references skip intermediates, feeding a projection back
    /// into [`splice`](Self::splice) keeps the graph shallow (bounded `resolve`
    /// indirection) and keeps the delete-liveness set to true data owners. Out-of-
    /// range bounds clamp like `resolve`; `start >= end` (after
    /// clamping) yields no segments. Same-owner but *non-consecutive* owner indices
    /// (e.g. a squash re-joining two ranges of one owner) correctly stay separate
    /// slices. A slice carries no provenance: the occurrences it names keep theirs
    /// in the owner. Derived at submit-/query-time, never per frame.
    pub fn project(&self, episode: &EpisodeId, start: usize, end: usize) -> Vec<Segment> {
        let resolved = self.resolve_annotated(episode, &mut Vec::new());
        let s = start.min(resolved.len());
        let e = end.min(resolved.len());
        if s >= e {
            return Vec::new();
        }
        let mut out: Vec<Segment> = Vec::new();
        for p in &resolved[s..e] {
            // Extend the trailing slice iff it references the same owner and this
            // turn is the very next owner-index; otherwise open a fresh slice.
            if let Some(Segment::Slice {
                episode: owner,
                end: run_end,
                ..
            }) = out.last_mut()
                && *owner == p.owner
                && *run_end == p.owner_index
            {
                *run_end = p.owner_index + 1;
            } else {
                out.push(Segment::Slice {
                    episode: p.owner,
                    start: p.owner_index,
                    end: p.owner_index + 1,
                });
            }
        }
        out
    }

    /// An episode's **full** head — the last turn of its derived sequence (which may
    /// be a shared-suffix turn for a re-root, not one of its own). `None` for an
    /// episode with no turns anywhere in its history, or an unknown id.
    pub fn head(&self, episode: &EpisodeId) -> Option<TurnId> {
        self.full_turns(episode).last().copied()
    }

    /// Mutable handle to an episode — the primitive a caller uses to grow it
    /// (e.g. the `Conversation` façade appending a freshly sealed turn). The
    /// richer write-side op-set (`branch`, `squash`, …) builds on this in P5.
    pub fn episode_mut(&mut self, id: &EpisodeId) -> Option<&mut Episode> {
        self.episodes.get_mut(id)
    }

    /// The one write **primitive** (design.md §1.2): create an episode from
    /// explicit [`Segment`]s. Every named op — [`branch`](Self::branch),
    /// [`branch_prefix`](Self::branch_prefix), [`re_root`](Self::re_root), and (P5)
    /// squash/graft — is a thin constructor over this, so the environment exposes
    /// one mechanism the named ops merely name. Inserts the episode and returns
    /// `new_id`. Slice ranges are validated *lazily* at resolution (clamped), so the
    /// primitive is infallible — the named ops compute in-bounds ranges.
    pub fn splice(
        &mut self,
        new_id: EpisodeId,
        config: TurnConfig,
        segments: Vec<Segment>,
        origin: Option<Origin>,
    ) -> EpisodeId {
        self.insert_episode(Episode::from_segments(new_id, config, segments, origin));
        new_id
    }

    /// Fork a **branch** (design.md §1.2 — the edit→branch op): a new episode that
    /// *shares* `parent`'s turns before `turn_index` (a leading prefix `Slice`) and
    /// holds `edited` as its sole own turn; turns *after* are dropped. Prefix-share,
    /// suffix-diverge — "change behavior moving forward." The origin
    /// ([`EdgeKind::Branch`]) pins the original turn at `turn_index` so the branch
    /// hangs off it. Inserts `edited` + the episode; `None` for a bad parent/index.
    pub fn branch(
        &mut self,
        parent_episode: EpisodeId,
        turn_index: usize,
        edited: Turn,
        new_id: EpisodeId,
    ) -> Option<EpisodeId> {
        let (config, fork_turn) = self.fork_anchor(&parent_episode, turn_index)?;
        let edited = self.admit(edited);
        let mut segments = self.project(&parent_episode, 0, turn_index);
        segments.push(Segment::Own(vec![edited]));
        Some(self.splice(
            new_id,
            config,
            segments,
            Some(Origin {
                episode: parent_episode,
                turn: fork_turn,
                index: Some(turn_index),
                kind: EdgeKind::Branch,
            }),
        ))
    }

    /// Fork a branch that shares `parent`'s prefix before `turn_index` and starts
    /// with no own turns — the caller appends whatever diverges (a re-run, a staged
    /// prompt). The empty-own shape the staged / regenerate edit modes use. `None`
    /// for a bad parent/index.
    pub fn branch_prefix(
        &mut self,
        parent_episode: EpisodeId,
        turn_index: usize,
        new_id: EpisodeId,
    ) -> Option<EpisodeId> {
        let (config, fork_turn) = self.fork_anchor(&parent_episode, turn_index)?;
        let segments = self.project(&parent_episode, 0, turn_index);
        Some(self.splice(
            new_id,
            config,
            segments,
            Some(Origin {
                episode: parent_episode,
                turn: fork_turn,
                index: Some(turn_index),
                kind: EdgeKind::Branch,
            }),
        ))
    }

    /// **Re-root** (design.md §1.2 — the *dual* of [`branch`](Self::branch)): graft
    /// `source`'s turns from `from` onward onto a **new root** `new_root`. The suffix
    /// is shared by reference (a trailing `Slice`, never copied); `new_root` is the
    /// only new data. Suffix-share, prefix-diverge — "change behavior at the base"
    /// (the user's true "clone"). The origin ([`EdgeKind::Clone`]) pins the first
    /// shared turn (the graft point). Inserts the new-root turns + the episode;
    /// `None` if `source` is unknown or `from` exceeds its length (`from == len` is
    /// allowed — a pure new root sharing nothing).
    pub fn re_root(
        &mut self,
        source: EpisodeId,
        from: usize,
        new_root: Vec<Turn>,
        config: TurnConfig,
        new_id: EpisodeId,
    ) -> Option<EpisodeId> {
        if !self.episodes.contains_key(&source) {
            return None;
        }
        let source_full = self.full_turns(&source);
        if from > source_full.len() {
            return None;
        }
        // The graft point (first shared turn) is the origin's attach turn; absent
        // when the re-root shares nothing (a bare new root).
        let origin = source_full.get(from).map(|&turn| Origin {
            episode: source,
            turn,
            index: Some(from),
            kind: EdgeKind::Clone,
        });
        let mut segments = Vec::new();
        if !new_root.is_empty() {
            segments.push(Segment::Own(
                new_root.into_iter().map(|t| self.admit(t)).collect(),
            ));
        }
        // The shared suffix, referencing the true data owners (like `branch`'s
        // prefix) — so a re-root of a derived source points at real data, not a
        // pass-through.
        segments.extend(self.project(&source, from, source_full.len()));
        Some(self.splice(new_id, config, segments, origin))
    }

    /// A fork point on `parent`: its config + the turn at `turn_index` of its full
    /// (derived) sequence. `None` if `parent` is unknown or the index is out of
    /// range — the shared bounds check for the prefix-fork ops.
    fn fork_anchor(&self, parent: &EpisodeId, turn_index: usize) -> Option<(TurnConfig, TurnId)> {
        let config = self.episodes.get(parent)?.config.clone();
        let fork_turn = *self.full_turns(parent).get(turn_index)?;
        Some((config, fork_turn))
    }

    /// The root nodes of the forest, as [`Node`]s — the top-level **sessions**:
    /// episodes with no origin. Everything with an origin — branch, clone, merge,
    /// tool-call — hangs under its origin turn via [`children`](Self::children),
    /// and ONLY there (one node, one place; clones/merges used to double-render
    /// as extra roots). Exception: an **orphan** whose origin episode is gone
    /// (parent deleted) falls back to a root — it would otherwise hang from a
    /// turn no row draws and silently vanish; visible-and-odd beats invisible.
    /// Sorted by id for deterministic traversal (the map is unordered).
    pub fn roots(&self) -> Vec<Node> {
        let mut ids: Vec<EpisodeId> = self
            .episodes
            .values()
            .filter(|e| {
                e.is_session_root()
                    || e.origin
                        .is_some_and(|o| !self.episodes.contains_key(&o.episode))
            })
            .map(|e| e.id)
            .collect();
        ids.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        ids.into_iter().map(Node::Episode).collect()
    }

    /// The children of a node — the one step every traversal repeats:
    ///
    /// - an **Episode**'s children are its **own** (divergent) turns, in order —
    ///   the shared prefix stays under the ancestor it's shared with (git-graph
    ///   style: shared history is drawn once, branches diverge from it);
    /// - a **Turn**'s children are its messages (in order) followed by any child
    ///   episodes (branches / sub-agents) forked *at this exact turn of this
    ///   episode*, sorted by id for determinism;
    /// - a **Message** is a leaf.
    ///
    /// Missing ids yield no children rather than panicking, so a partially
    /// hydrated graph still walks.
    pub fn children(&self, node: Node) -> Vec<Node> {
        match node {
            // An episode's children are its **own** (divergent) turns — the shared
            // prefix stays under the ancestor it's shared with. Each carries its
            // position in the FULL derived sequence (its occurrence index), found by
            // enumerating the owner-tagged sequence rather than searching content.
            Node::Episode(id) => self
                .placed(&id)
                .into_iter()
                .enumerate()
                .filter(|(_, p)| p.owner == id)
                .map(|(index, p)| Node::Turn {
                    episode: id,
                    turn: p.turn,
                    index,
                })
                .collect(),
            Node::Turn {
                episode,
                turn,
                index: turn_index,
            } => {
                let mut out = Vec::new();
                // The outline, not the body: listing a turn's messages must not
                // need them loaded.
                if let Some(outline) = self.outline(&turn) {
                    for index in 0..outline.len() {
                        out.push(Node::Message {
                            episode,
                            turn,
                            turn_index,
                            index,
                        });
                    }
                }
                // Child episodes born at exactly THIS OCCURRENCE of this turn.
                // Content-addressed ids recur; matching by id alone hung a child
                // under every duplicate. A legacy origin (`index: None`) resolves
                // to the id's first occurrence — computed once here (view-build
                // time, not a per-frame path).
                let first_occurrence = self.full_turns(&episode).iter().position(|t| *t == turn);
                let mut kids: Vec<EpisodeId> = self
                    .episodes
                    .values()
                    .filter(|e| {
                        e.origin.is_some_and(|o| {
                            o.episode == episode
                                && o.turn == turn
                                && o.index.or(first_occurrence) == Some(turn_index)
                        })
                    })
                    .map(|e| e.id)
                    .collect();
                kids.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
                out.extend(kids.into_iter().map(Node::Episode));
                out
            }
            Node::Message { .. } => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::episode::{EdgeKind, Origin};
    use crate::graph::turn::TurnConfig;
    use crate::message::Message;
    use std::time::SystemTime;

    fn cfg() -> TurnConfig {
        TurnConfig {
            model: "claude-haiku".into(),
            max_tokens: None,
            effort: None,
        }
    }

    /// Seal a 2-message turn (user + assistant) keyed off `tag`.
    fn turn(tag: &str) -> Turn {
        Turn::seal(
            vec![Message::user(tag), Message::assistant(tag)],
            cfg(),
            SystemTime::UNIX_EPOCH,
        )
    }

    /// **A message of calls or results outlines as what it holds** — a
    /// turn's tool rounds were rows reading `asst:` and `user:` over nothing
    /// (Steven, 10-16). Words come first, a text block's too; with none, a
    /// call by its tool and first argument, a result by its first line, in
    /// brackets as no one's words.
    #[test]
    fn a_message_without_words_outlines_as_what_it_holds() {
        let holding = |role, blocks| Message {
            role,
            content: String::new(),
            blocks,
        };
        let call = |id: &str, name: &str, input| Block::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        };
        let result = |content: &str, is_error| Block::ToolResult {
            tool_use_id: "t".into(),
            content: content.into(),
            is_error,
        };
        let lines: Vec<(String, bool)> = outline_of(&[
            holding(
                Role::Assistant,
                vec![Block::bash_call("t1", "\ncat .env\nls")],
            ),
            holding(
                Role::Assistant,
                vec![
                    call("t2", "Read", serde_json::json!({"file_path": "/a"})),
                    Block::bash_call("t3", "ls"),
                    call("t4", "skill", serde_json::json!({})),
                ],
            ),
            holding(Role::User, vec![result("KEY=1\nOTHER=2", false)]),
            holding(Role::User, vec![result("", false), result("x", true)]),
            holding(Role::User, vec![result("no such file", true)]),
            holding(
                Role::Assistant,
                vec![
                    Block::Text {
                        text: "\n  Let me look.".into(),
                    },
                    Block::bash_call("t5", "ls"),
                ],
            ),
            holding(Role::User, Vec::new()),
        ])
        .into_iter()
        .map(|o| (o.first_line, o.words))
        .collect();
        let expected = [
            ("[bash: cat .env]", false),
            ("[Read: /a, and 2 more]", false),
            ("[result: KEY=1]", false),
            ("[result: empty, and 1 more]", false),
            ("[error: no such file]", false),
            ("Let me look.", true),
            ("", true),
        ];
        assert_eq!(
            lines,
            expected.map(|(line, words)| (line.to_string(), words))
        );
    }

    /// Preorder DFS over the one recursive `children` step, collecting nodes.
    fn preorder(g: &Graph, node: Node, out: &mut Vec<Node>) {
        out.push(node);
        for c in g.children(node) {
            preorder(g, c, out);
        }
    }

    /// `tag`'s turn as one PRODUCTION: the same content (so the same id) as
    /// every other production of `tag`, with its own time and bill.
    fn produced(tag: &str, secs: u64, output_tokens: u32) -> Turn {
        let mut t = Turn::seal(
            vec![Message::user(tag), Message::assistant(tag)],
            cfg(),
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs),
        );
        t.provenance.usage = Some(crate::Usage {
            output_tokens,
            ..Default::default()
        });
        t
    }

    /// **Two productions of one turn keep their own facts.** The content is
    /// one turn (one id, stored once); when each ran and what each cost are
    /// not. Keyed by id, the second production overwrote the first's
    /// provenance — the bug this split exists to make unrepresentable.
    #[test]
    fn identical_turns_keep_their_own_provenance() {
        let (first, second) = (produced("hi", 1, 10), produced("hi", 2, 20));
        assert_eq!(first.id, second.id, "premise: one content, one id");
        let mut g = Graph::new();
        let id = EpisodeId::from_seed(b"twice");
        let mut ep = Episode::root(id, cfg());
        ep.append(g.admit(first.clone()));
        ep.append(g.admit(second.clone()));
        g.insert_episode(ep);

        let placed = g.placed(&id);
        assert_eq!(placed[0].provenance, &first.provenance);
        assert_eq!(placed[1].provenance, &second.provenance);
        assert!(!placed[0].same_occurrence(&placed[1]), "two occurrences");
        assert_eq!(
            g.records(&id),
            Some(vec![first, second]),
            "each record, its own facts"
        );
    }

    /// Shared history carries the facts of where it was produced: a clone that
    /// slices a turn reads its owner's occurrence, not a later production of
    /// the same content somewhere else.
    #[test]
    fn a_slice_reads_its_owners_occurrence() {
        let (original, redo) = (produced("hi", 1, 10), produced("hi", 2, 20));
        let mut g = Graph::new();
        let root = EpisodeId::from_seed(b"root");
        let mut ep = Episode::root(root, cfg());
        ep.append(g.admit(original.clone()));
        g.insert_episode(ep);
        let other = EpisodeId::from_seed(b"other");
        let mut ep = Episode::root(other, cfg());
        ep.append(g.admit(redo.clone()));
        g.insert_episode(ep);
        let clone = EpisodeId::from_seed(b"clone");
        g.splice(clone, cfg(), g.project(&root, 0, 1), None);

        assert_eq!(g.records(&clone), Some(vec![original]), "the owner's facts");
        assert_eq!(
            g.records(&other),
            Some(vec![redo]),
            "and the redo keeps its own"
        );
        assert!(g.placed(&clone)[0].same_occurrence(&g.placed(&root)[0]));
    }

    /// **What a composition's sources vouch for**: every occurrence of the
    /// asked content — the same turn sealed in two runs records differently,
    /// and either was a real hold — and nothing another turn recorded, nor
    /// anything from a legacy occurrence that recorded nothing.
    #[test]
    fn recorded_handles_are_every_occurrence_of_the_asked_turns() {
        let first = produced("hi", 1, 10).with_handles(vec!["{{a.0001}}".into()]);
        let second = produced("hi", 2, 20).with_handles(vec!["{{b.0002}}".into()]);
        let mut legacy = produced("hi", 3, 30);
        legacy.provenance.handles = None;
        let other = turn("other").with_handles(vec!["{{c.0003}}".into()]);
        let mut g = Graph::new();
        for (seed, turns) in [
            (&b"one"[..], vec![first.clone(), other.clone()]),
            (&b"two"[..], vec![second, legacy]),
        ] {
            let mut ep = Episode::root(EpisodeId::from_seed(seed), cfg());
            for t in turns {
                ep.append(g.admit(t));
            }
            g.insert_episode(ep);
        }
        assert_eq!(
            g.recorded_handles(&[first.id]),
            vec!["{{a.0001}}".to_string(), "{{b.0002}}".to_string()]
        );
        assert_eq!(g.recorded_handles(&[other.id]), vec!["{{c.0003}}"]);
        assert!(g.recorded_handles(&[]).is_empty(), "no sources, no vouch");
    }

    /// Removing a turn rebuilds the episode's own segment; every kept
    /// occurrence keeps its facts — including the twin of the one removed.
    #[test]
    fn remove_turns_keeps_each_occurrences_provenance() {
        let (x1, y, x2) = (produced("x", 1, 10), turn("y"), produced("x", 3, 30));
        let mut g = Graph::new();
        let id = EpisodeId::from_seed(b"e");
        let mut ep = Episode::root(id, cfg());
        for t in [&x1, &y, &x2] {
            ep.append(g.admit(t.clone()));
        }
        g.insert_episode(ep);
        assert!(g.remove_turns(&id, &[0]));
        assert_eq!(
            g.records(&id),
            Some(vec![y, x2]),
            "the second x keeps ITS bill"
        );
    }

    /// What a reader must fetch: each missing body once, in order, and nothing
    /// already loaded — a repeated turn is one body to load, not two.
    #[test]
    fn unloaded_names_each_missing_body_once() {
        let (x, y, z) = (turn("x"), turn("y"), turn("z"));
        let mut g = Graph::new();
        for t in [&x, &y] {
            g.insert_outline(t.id, cfg().model, outline_of(&t.messages));
        }
        g.insert_content(z.clone());
        assert_eq!(
            g.unloaded([y.id, x.id, z.id, y.id, x.id]),
            vec![y.id, x.id],
            "outline-only turns, each once; the loaded one is not asked for"
        );
        g.fill_bodies([(x.id, x.messages.clone())]);
        assert_eq!(g.unloaded([x.id, y.id]), vec![y.id], "filled is loaded");
    }

    #[test]
    fn a_child_anchors_at_one_occurrence_of_a_repeated_turn() {
        // The content-id trap, third strike (P6.0 regression gate): a parent with
        // the SAME turn content twice — one TurnId, two occurrences. A child
        // anchored at occurrence 1 must appear under row 1 and ONLY row 1; a
        // legacy origin (index: None) resolves to the FIRST occurrence only.
        // Pre-fix, id-only matching hung the child under BOTH rows.
        let a = turn("a");
        let root_id = EpisodeId::from_seed(b"root");
        let mut root = Episode::root(root_id, cfg());
        root.append(a.occurrence());
        root.append(a.occurrence()); // identical content — same id, second occurrence
        let mut g = Graph::new();
        g.insert_content(a.clone());
        g.insert_episode(root);

        let indexed = EpisodeId::from_seed(b"indexed");
        let legacy = EpisodeId::from_seed(b"legacy");
        for (id, index) in [(indexed, Some(1)), (legacy, None)] {
            let mut child = Episode::branch(
                id,
                cfg(),
                Origin {
                    episode: root_id,
                    turn: a.id,
                    index,
                    kind: EdgeKind::Branch,
                },
                index.map_or(1, |i| i + 1),
            );
            let c = turn("c");
            g.insert_content(c.clone());
            child.append(c.occurrence());
            g.insert_episode(child);
        }

        let row = |index| Node::Turn {
            episode: root_id,
            turn: a.id,
            index,
        };
        let kids0 = g.children(row(0));
        let kids1 = g.children(row(1));
        assert!(
            kids1.contains(&Node::Episode(indexed)) && !kids0.contains(&Node::Episode(indexed)),
            "an indexed origin anchors at ITS occurrence only"
        );
        assert!(
            kids0.contains(&Node::Episode(legacy)) && !kids1.contains(&Node::Episode(legacy)),
            "a legacy (index-less) origin anchors at the FIRST occurrence only"
        );
    }

    #[test]
    fn depth_and_spawn_children_walk_the_ancestry_tree() {
        // The ancestry helpers guardrails use: depth = origin-chain length;
        // spawn_children = out-edges. These walk the ORIGIN tree, never the
        // content-sharing (Slice) DAG.
        let (a, b) = (turn("a"), turn("b"));
        let root_id = EpisodeId::from_seed(b"root");
        let mut root = Episode::root(root_id, cfg());
        root.append(a.occurrence());
        root.append(b.occurrence());
        let mut g = Graph::new();
        g.insert_content(a.clone());
        g.insert_content(b.clone());
        g.insert_episode(root);
        let child = EpisodeId::from_seed(b"child");
        g.branch_prefix(root_id, 1, child).unwrap(); // shares [a], forks at b
        let c = turn("c");
        g.insert_content(c.clone());
        g.episode_mut(&child).unwrap().append(c.occurrence());
        let grandchild = EpisodeId::from_seed(b"grand");
        g.branch_prefix(child, 1, grandchild).unwrap(); // forks at child's own c

        assert_eq!(g.depth(&root_id), 0, "a root has no ancestors");
        assert_eq!(g.depth(&child), 1);
        assert_eq!(g.depth(&grandchild), 2);
        assert_eq!(g.spawn_children(&root_id), vec![child]);
        assert_eq!(g.spawn_children(&child), vec![grandchild]);
        assert!(g.spawn_children(&grandchild).is_empty());
    }

    #[test]
    fn child_metrics_count_only_toolcall_edges() {
        // The guardrail metrics vs. the general ancestry helpers: a chain of
        // root → (Branch) → (ToolCall) → (ToolCall) is 3 deep in ancestry but
        // only 2 spawns deep — user surgery must never eat into the agent
        // recursion budget. Same for width: the root's Branch child is not a
        // child session.
        let a = turn("a");
        let root_id = EpisodeId::from_seed(b"root");
        let mut root = Episode::root(root_id, cfg());
        root.append(a.occurrence());
        let mut g = Graph::new();
        g.insert_content(a.clone());
        g.insert_episode(root);

        let chain = [
            (EpisodeId::from_seed(b"branch"), EdgeKind::Branch),
            (EpisodeId::from_seed(b"spawn1"), EdgeKind::ToolCall),
            (EpisodeId::from_seed(b"spawn2"), EdgeKind::ToolCall),
        ];
        let mut parent = root_id;
        for (id, kind) in chain {
            // Each link hangs a fresh (own-turn) episode off the parent's
            // first turn — the ancestry edge is what's under test.
            let ep = Episode::from_segments(
                id,
                cfg(),
                vec![],
                Some(Origin {
                    episode: parent,
                    turn: a.id,
                    index: Some(0),
                    kind,
                }),
            );
            g.insert_episode(ep);
            parent = id;
        }

        let (branch, spawn1, spawn2) = (chain[0].0, chain[1].0, chain[2].0);
        assert_eq!(g.depth(&spawn2), 3, "ancestry counts every edge");
        assert_eq!(g.child_depth(&branch), 0, "a branch is not a spawn");
        assert_eq!(g.child_depth(&spawn1), 1);
        assert_eq!(g.child_depth(&spawn2), 2, "only ToolCall edges accumulate");
        assert_eq!(
            g.child_sessions(&root_id),
            vec![],
            "the root's Branch child is not a child session"
        );
        assert_eq!(g.child_sessions(&branch), vec![spawn1]);
        assert_eq!(g.spawn_children(&branch), vec![spawn1]);
    }

    #[test]
    fn one_traversal_walks_graph_episode_turn_message_and_subepisode() {
        // Parent episode with two turns A, B; a child episode branched at B
        // with its own turn C. The single `children` step must reach every
        // level — episode, turn, message — and descend into the child episode.
        let (a, b, c) = (turn("a"), turn("b"), turn("c"));
        let root_id = EpisodeId::from_seed(b"root");
        let child_id = EpisodeId::from_seed(b"child");

        let mut root = Episode::root(root_id, cfg());
        root.append(a.occurrence());
        root.append(b.occurrence());

        let mut child = Episode::branch(
            child_id,
            cfg(),
            Origin {
                episode: root_id,
                turn: b.id,
                index: Some(1),
                kind: EdgeKind::Branch,
            },
            2, // shares [a, b]; diverges with its own c
        );
        child.append(c.occurrence());

        let mut g = Graph::new();
        g.insert_content(a.clone());
        g.insert_content(b.clone());
        g.insert_content(c.clone());
        g.insert_episode(root);
        g.insert_episode(child);

        // Only the parent is a forest root; the branch (leading Slice) is reached
        // *through* the graph.
        assert_eq!(g.roots(), vec![Node::Episode(root_id)]);

        let mut visited = Vec::new();
        preorder(&g, Node::Episode(root_id), &mut visited);

        // Every level is represented, and the sub-episode is reached via B.
        assert!(visited.contains(&Node::Episode(root_id)));
        assert!(visited.contains(&Node::Turn {
            episode: root_id,
            turn: a.id,
            index: 0,
        }));
        assert!(visited.contains(&Node::Message {
            episode: root_id,
            turn: a.id,
            turn_index: 0,
            index: 1,
        }));
        assert!(
            visited.contains(&Node::Episode(child_id)),
            "the sub-episode is reached through the parent's turn"
        );
        assert!(visited.contains(&Node::Message {
            episode: child_id,
            turn: c.id,
            turn_index: 2,
            index: 0,
        }));

        // Full preorder, with the child episode hanging off B (the branch
        // point): root, [A, A.0, A.1], [B, B.0, B.1, [child, C, C.0, C.1]].
        // root(1) + A-subtree(3) + B-subtree(3) + child-subtree(4) = 11.
        assert_eq!(visited.len(), 1 + 3 + 3 + 4);
    }

    #[test]
    fn a_message_is_a_leaf() {
        let mut g = Graph::new();
        let t = turn("x");
        let ep_id = EpisodeId::from_seed(b"e");
        let mut ep = Episode::root(ep_id, cfg());
        ep.append(t.occurrence());
        g.insert_content(t.clone());
        g.insert_episode(ep);

        let msg = Node::Message {
            episode: ep_id,
            turn: t.id,
            turn_index: 0,
            index: 0,
        };
        assert!(g.children(msg).is_empty());
    }

    #[test]
    fn branch_attaches_only_at_its_exact_turn() {
        // Root has turns A, B; branch forks at A. The child must appear under
        // A's children, never B's — the episode-qualified node makes this exact.
        let (a, b, c) = (turn("a"), turn("b"), turn("c"));
        let root_id = EpisodeId::from_seed(b"root");
        let child_id = EpisodeId::from_seed(b"child");

        let mut root = Episode::root(root_id, cfg());
        root.append(a.occurrence());
        root.append(b.occurrence());
        let mut child = Episode::branch(
            child_id,
            cfg(),
            Origin {
                episode: root_id,
                turn: a.id,
                index: Some(0),
                kind: EdgeKind::Branch,
            },
            1, // shares [a]; diverges with its own c
        );
        child.append(c.occurrence());

        let mut g = Graph::new();
        g.insert_content(a.clone());
        g.insert_content(b.clone());
        g.insert_content(c.clone());
        g.insert_episode(root);
        g.insert_episode(child);

        let at_a = g.children(Node::Turn {
            episode: root_id,
            turn: a.id,
            index: 0,
        });
        let at_b = g.children(Node::Turn {
            episode: root_id,
            turn: b.id,
            index: 1,
        });
        assert!(at_a.contains(&Node::Episode(child_id)), "forked at A");
        assert!(!at_b.contains(&Node::Episode(child_id)), "not under B");
    }

    #[test]
    fn missing_ids_yield_no_children() {
        let g = Graph::new();
        assert!(
            g.children(Node::Episode(EpisodeId::from_seed(b"nope")))
                .is_empty()
        );
        assert!(g.roots().is_empty());
    }

    #[test]
    fn branch_shares_the_prefix_swaps_the_edited_turn_and_drops_the_rest() {
        // root: a, b, c. Edit turn index 1 (b) → branch shares [a], diverges with
        // the edited turn as its sole own turn; c is dropped.
        let (a, b, c) = (turn("a"), turn("b"), turn("c"));
        let root_id = EpisodeId::from_seed(b"root");
        let mut root = Episode::root(root_id, cfg());
        root.append(a.occurrence());
        root.append(b.occurrence());
        root.append(c.occurrence());

        let mut g = Graph::new();
        g.insert_content(a.clone());
        g.insert_content(b.clone());
        g.insert_content(c.clone());
        g.insert_episode(root);

        let edited = turn("b-edited");
        let branch_id = EpisodeId::from_seed(b"branch");
        assert_eq!(
            g.branch(root_id, 1, edited.clone(), branch_id),
            Some(branch_id)
        );

        let branch = g.episode(&branch_id).unwrap();
        assert_eq!(
            branch.own_turns(),
            vec![edited.id],
            "the branch owns only the edited turn — the prefix is shared, not copied"
        );
        assert_eq!(
            branch.segments(),
            &[
                Segment::Slice {
                    episode: root_id,
                    start: 0,
                    end: 1,
                },
                Segment::Own(vec![edited.occurrence()]),
            ],
            "a prefix Slice of [a] then the edited own turn"
        );
        assert_eq!(
            g.full_turns(&branch_id),
            vec![a.id, edited.id],
            "full history derives as shared [a] + the edited turn; c is dropped"
        );
        let origin = branch.origin.unwrap();
        assert_eq!(origin.episode, root_id);
        assert_eq!(origin.turn, b.id, "the origin pins the original turn");
        assert_eq!(origin.kind, EdgeKind::Branch);

        // It hangs off turn b of root in the tree traversal.
        let under_b = g.children(Node::Turn {
            episode: root_id,
            turn: b.id,
            index: 1,
        });
        assert!(
            under_b.contains(&Node::Episode(branch_id)),
            "branch hangs off the forked turn"
        );

        // The edited turn is in the graph; the original c is untouched.
        assert!(g.messages(&edited.id).is_some());
        assert!(g.messages(&c.id).is_some());
    }

    #[test]
    fn branch_rejects_a_bad_parent_or_index() {
        let mut g = Graph::new();
        let root_id = EpisodeId::from_seed(b"root");
        let mut root = Episode::root(root_id, cfg());
        let a = turn("a");
        root.append(a.occurrence());
        g.insert_content(a);
        g.insert_episode(root);

        let edited = turn("x");
        let branch_id = EpisodeId::from_seed(b"branch");
        assert_eq!(
            g.branch(EpisodeId::from_seed(b"ghost"), 0, edited.clone(), branch_id),
            None,
            "unknown parent"
        );
        assert_eq!(
            g.branch(root_id, 5, edited, branch_id),
            None,
            "index out of range"
        );
    }

    #[test]
    fn episodes_with_turn_counts_shared_content_addressed_turns() {
        // A turn with identical content recurs across episodes (dedup) — the
        // count is how the inspector explains "this turn is in N sessions".
        let shared = turn("shared");
        let only = turn("only");
        let (e1, e2) = (EpisodeId::from_seed(b"e1"), EpisodeId::from_seed(b"e2"));
        let mut ep1 = Episode::root(e1, cfg());
        ep1.append(shared.occurrence());
        let mut ep2 = Episode::root(e2, cfg());
        ep2.append(shared.occurrence());
        ep2.append(only.occurrence());

        let mut g = Graph::new();
        g.insert_content(shared.clone());
        g.insert_content(only.clone());
        g.insert_episode(ep1);
        g.insert_episode(ep2);

        assert_eq!(
            g.episodes_with_turn(&shared.id),
            2,
            "shared turn is in both"
        );
        assert_eq!(g.episodes_with_turn(&only.id), 1);
        assert_eq!(g.episode_ids().len(), 2);
    }

    #[test]
    fn branch_prefix_shares_only_the_turns_before_the_fork() {
        let (a, b, c) = (turn("a"), turn("b"), turn("c"));
        let root_id = EpisodeId::from_seed(b"root");
        let mut root = Episode::root(root_id, cfg());
        root.append(a.occurrence());
        root.append(b.occurrence());
        root.append(c.occurrence());
        let mut g = Graph::new();
        g.insert_content(a.clone());
        g.insert_content(b.clone());
        g.insert_content(c.clone());
        g.insert_episode(root);

        let branch_id = EpisodeId::from_seed(b"branch");
        assert_eq!(g.branch_prefix(root_id, 1, branch_id), Some(branch_id));
        let branch = g.episode(&branch_id).unwrap();
        assert!(
            branch.own_is_empty(),
            "nothing committed yet in place of the edited turn"
        );
        assert_eq!(
            g.full_turns(&branch_id),
            vec![a.id],
            "the shared prefix [a] derives from the parent; nothing copied"
        );
        assert_eq!(
            branch.segments(),
            &[Segment::Slice {
                episode: root_id,
                start: 0,
                end: 1,
            }],
            "just the prefix Slice, no own segment yet"
        );
        assert_eq!(branch.origin.unwrap().turn, b.id, "forks at b");
    }

    /// **An orphan is a root** — a branch whose origin episode is gone (its
    /// parent deleted) is drawn at the top rather than hung from a turn no row
    /// draws, where it would vanish; with its origin here it is not (the
    /// forest test above). Held only by two TUI tests (review stop 12, T9).
    #[test]
    fn a_branch_whose_origin_is_gone_is_a_root() {
        let (a, b) = (turn("a"), turn("b"));
        let orphan_id = EpisodeId::from_seed(b"orphan");
        let mut orphan = Episode::branch(
            orphan_id,
            cfg(),
            Origin {
                episode: EpisodeId::from_seed(b"gone"),
                turn: a.id,
                index: Some(0),
                kind: EdgeKind::Branch,
            },
            1,
        );
        orphan.append(b.occurrence());
        let mut g = Graph::new();
        g.insert_content(a);
        g.insert_content(b);
        g.insert_episode(orphan);
        assert_eq!(g.roots(), vec![Node::Episode(orphan_id)]);
    }

    /// A root episode a/b/c for the op tests.
    fn abc_root() -> (Graph, EpisodeId, Turn, Turn, Turn) {
        let (a, b, c) = (turn("a"), turn("b"), turn("c"));
        let root_id = EpisodeId::from_seed(b"root");
        let mut root = Episode::root(root_id, cfg());
        root.append(a.occurrence());
        root.append(b.occurrence());
        root.append(c.occurrence());
        let mut g = Graph::new();
        g.insert_content(a.clone());
        g.insert_content(b.clone());
        g.insert_content(c.clone());
        g.insert_episode(root);
        (g, root_id, a, b, c)
    }

    #[test]
    fn remove_turns_keeps_the_episode_id_and_its_own_data() {
        // The whole point of in-place removal: the session is still the session.
        // Composing instead would mint a new id, and the row the user was
        // looking at would vanish and be replaced by a stranger.
        let (mut g, root_id, a, _b, c) = abc_root();
        assert!(g.remove_turns(&root_id, &[1]));
        assert_eq!(g.full_turns(&root_id), vec![a.id, c.id]);
        assert_eq!(
            g.episode(&root_id).unwrap().own_turns(),
            vec![a.id, c.id],
            "a root owns its turns, and must still own them afterwards — \
             lowering them to slices would point the episode at itself"
        );
    }

    #[test]
    fn removing_nothing_is_the_identity() {
        // The rebuild reconstructs every segment, so "drop none" is the case
        // that proves it reconstructs them FAITHFULLY. A coalescing bug shows up
        // here as a shape change with no content change, which no
        // turn-sequence assertion alone would catch.
        let (mut g, root_id, ..) = abc_root();
        let before_turns = g.full_turns(&root_id);
        let before_segments = g.episode(&root_id).unwrap().segments().to_vec();
        assert!(g.remove_turns(&root_id, &[]));
        assert_eq!(g.full_turns(&root_id), before_turns);
        assert_eq!(
            g.episode(&root_id).unwrap().segments(),
            before_segments.as_slice(),
            "an empty removal must not reshape the piece table"
        );
    }

    #[test]
    fn remove_turns_preserves_borrowed_turns_as_slices() {
        // A branch shares its parent's prefix by reference. Dropping one of its
        // OWN turns must leave the borrowed prefix borrowed — rewriting it as
        // `Own` would silently copy another episode's data into this one and
        // double its storage, breaking "share unless changed".
        let (mut g, root_id, a, b, _c) = abc_root();
        let branch_id = EpisodeId::from_seed(b"branch");
        let (d, e) = (turn("d"), turn("e"));
        g.branch_prefix(root_id, 2, branch_id); // shares [a, b]
        g.insert_content(d.clone());
        g.insert_content(e.clone());
        {
            let ep = g.episodes.get_mut(&branch_id).unwrap();
            ep.append(d.occurrence());
            ep.append(e.occurrence());
        }
        assert_eq!(g.full_turns(&branch_id), vec![a.id, b.id, d.id, e.id]);

        assert!(g.remove_turns(&branch_id, &[2])); // drop `d`, its own
        assert_eq!(g.full_turns(&branch_id), vec![a.id, b.id, e.id]);
        assert_eq!(
            g.episode(&branch_id).unwrap().own_turns(),
            vec![e.id],
            "only `e` is still ours"
        );
        assert!(
            g.episode(&branch_id).unwrap().segments().iter().any(
                |s| matches!(s, Segment::Slice { episode, start: 0, end: 2 } if *episode == root_id)
            ),
            "the shared prefix is still shared by reference: {:?}",
            g.episode(&branch_id).unwrap().segments()
        );
    }

    /// **Detaching moves where the data lives, never what is read.** A fork
    /// of a fork: `y` reads `a b` (owned by the root), `d` (owned by `x`) and
    /// its own `e`. Detached from `x`, `d` is `y`'s own — the same occurrence
    /// — while `a b` stay shared with the root, by reference; nothing of `y`
    /// names `x` any more, so `x` can go.
    #[test]
    fn detach_owns_what_was_borrowed_from_one_episode_only() {
        let (mut g, root_id, a, b, _c) = abc_root();
        let (x, y) = (EpisodeId::from_seed(b"x"), EpisodeId::from_seed(b"y"));
        let (d, e) = (turn("d"), turn("e"));
        g.branch_prefix(root_id, 2, x); // [a, b]
        g.insert_content(d.clone());
        g.episodes.get_mut(&x).unwrap().append(d.occurrence());
        // `y` slices `x` itself — an intermediate, as `Episode::branch` makes
        // — not the owners `project` would name.
        let config = g.episode(&x).unwrap().config.clone();
        let through_x = vec![Segment::Slice {
            episode: x,
            start: 0,
            end: 3,
        }];
        g.insert_episode(Episode::from_segments(y, config, through_x, None));
        g.insert_content(e.clone());
        g.episodes.get_mut(&y).unwrap().append(e.occurrence());
        let before = g.full_turns(&y);
        let provenance = g.placed(&y)[2].provenance.clone();
        assert_eq!(before, vec![a.id, b.id, d.id, e.id], "premise");

        assert!(g.detach(&y, &x));
        assert_eq!(g.full_turns(&y), before, "the same history");
        let ep = g.episode(&y).unwrap();
        assert_eq!(ep.own_turns(), vec![d.id, e.id], "`d` is y's now");
        assert_eq!(
            *g.placed(&y)[2].provenance,
            provenance,
            "the same occurrence"
        );
        assert!(
            ep.segments()
                .iter()
                .all(|s| !matches!(s, Segment::Slice { episode, .. } if *episode == x)),
            "nothing of y names x: {:?}",
            ep.segments()
        );
        assert!(
            ep.segments().iter().any(
                |s| matches!(s, Segment::Slice { episode, start: 0, end: 2 } if *episode == root_id)
            ),
            "what the root owns stays shared: {:?}",
            ep.segments()
        );
        assert!(!g.detach(&EpisodeId::from_seed(b"absent"), &x));
    }

    /// **A rewrite keeps what the session carries** (review stop 2, A F1):
    /// its name and import record survive a turn delete and a detach. Where
    /// it parted from Claude Code moves down by each imported turn dropped —
    /// once, however often `drop` names it — and not for a turn after it.
    #[test]
    fn a_rewrite_keeps_the_sessions_name_and_import_record() {
        use crate::graph::episode::Inherited;
        let (mut g, root_id, ..) = abc_root();
        {
            let ep = g.episodes.get_mut(&root_id).unwrap();
            ep.label.name = Some("went-on".into());
            ep.inherited = Some(Inherited {
                session: "cc".into(),
                turns: 2,
                at: Some("u1".into()),
                opened: vec![Some("u0".into()), Some("u1".into())],
            });
        }
        let carried = |g: &Graph| {
            let ep = g.episode(&root_id).unwrap();
            (
                ep.label.name.clone(),
                ep.inherited.as_ref().map(|i| (i.turns, i.opened.len())),
            )
        };
        assert!(g.remove_turns(&root_id, &[2]), "a turn after the parting");
        assert_eq!(carried(&g), (Some("went-on".into()), Some((2, 2))));
        assert!(g.remove_turns(&root_id, &[0, 0]), "an imported one, twice");
        assert_eq!(
            carried(&g),
            (Some("went-on".into()), Some((1, 0))),
            "no place recorded, each turn's with it"
        );

        let (mut g, root_id, ..) = abc_root();
        let x = EpisodeId::from_seed(b"x");
        g.branch_prefix(root_id, 2, x);
        g.episodes.get_mut(&x).unwrap().label.name = Some("clone".into());
        assert!(g.detach(&x, &root_id));
        assert_eq!(g.episode(&x).unwrap().label.name.as_deref(), Some("clone"));
    }

    #[test]
    fn remove_turns_drops_an_occurrence_not_every_copy_of_the_content() {
        // Content-addressing recurs: the same turn can sit at two positions.
        // `drop` is positional, so removing one must leave the other
        // ([[feedback_content_addressed_ids]] — an id is not an occurrence).
        let dup = turn("same");
        let other = turn("other");
        let id = EpisodeId::from_seed(b"dup");
        let mut ep = Episode::root(id, cfg());
        ep.append(dup.occurrence());
        ep.append(other.occurrence());
        ep.append(dup.occurrence());
        let mut g = Graph::new();
        g.insert_content(dup.clone());
        g.insert_content(other.clone());
        g.insert_episode(ep);
        assert_eq!(g.full_turns(&id), vec![dup.id, other.id, dup.id]);

        assert!(g.remove_turns(&id, &[0]));
        assert_eq!(
            g.full_turns(&id),
            vec![other.id, dup.id],
            "the second occurrence survives — we deleted a position, not content"
        );
    }

    #[test]
    fn remove_turns_ignores_out_of_range_and_unknown_episodes() {
        let (mut g, root_id, a, b, c) = abc_root();
        assert!(g.remove_turns(&root_id, &[99]));
        assert_eq!(g.full_turns(&root_id), vec![a.id, b.id, c.id]);
        assert!(
            !g.remove_turns(&EpisodeId::from_seed(b"ghost"), &[0]),
            "an unknown episode is a no-op, not a panic and not a new episode"
        );
    }

    #[test]
    fn re_root_grafts_a_new_root_and_shares_the_suffix() {
        // The dual of branch: keep the SUFFIX [b, c], graft it onto a new root
        // [x, y]. The new root is the only new data; the suffix is shared by id.
        let (mut g, root_id, a, b, c) = abc_root();
        let (x, y) = (turn("x"), turn("y"));
        let clone_id = EpisodeId::from_seed(b"clone");

        assert_eq!(
            g.re_root(root_id, 1, vec![x.clone(), y.clone()], cfg(), clone_id),
            Some(clone_id)
        );
        let clone = g.episode(&clone_id).unwrap();
        assert_eq!(
            g.full_turns(&clone_id),
            vec![x.id, y.id, b.id, c.id],
            "new root [x, y] then the shared suffix [b, c]"
        );
        assert_eq!(
            clone.own_turns(),
            vec![x.id, y.id],
            "only the new root is new data — the suffix is borrowed"
        );
        assert_eq!(
            clone.segments(),
            &[
                Segment::Own(vec![x.occurrence(), y.occurrence()]),
                Segment::Slice {
                    episode: root_id,
                    start: 1,
                    end: 3,
                },
            ]
        );
        // One node, one place: the re-root hangs under its graft point (its
        // origin turn), NOT as a second forest root — but it DISPLAYS its full
        // derived history (it owns the new root; the suffix reads through).
        assert!(!g.roots().contains(&Node::Episode(clone_id)));
        assert!(g.episode(&clone_id).unwrap().shows_full_history());
        assert_eq!(
            g.head(&clone_id),
            Some(c.id),
            "tip is the shared suffix's end"
        );
        let origin = clone.origin.unwrap();
        assert_eq!(
            origin.turn, b.id,
            "origin pins the graft point (first shared turn)"
        );
        assert_eq!(origin.kind, EdgeKind::Clone);
        // The suffix turns are shared, not duplicated: b/c are in root AND the clone.
        assert_eq!(g.episodes_with_turn(&b.id), 2);
        assert_eq!(g.episodes_with_turn(&c.id), 2);
        assert_eq!(
            g.episodes_with_turn(&a.id),
            1,
            "a was dropped from the clone"
        );
        assert_eq!(
            g.episodes_with_turn(&x.id),
            1,
            "the new root is only on the clone"
        );
    }

    #[test]
    fn re_root_sharing_nothing_is_a_bare_new_root() {
        let (mut g, root_id, ..) = abc_root();
        let x = turn("x");
        let id = EpisodeId::from_seed(b"bare");
        // `from == len` shares no suffix — just a fresh root with new data.
        assert_eq!(g.re_root(root_id, 3, vec![x.clone()], cfg(), id), Some(id));
        let ep = g.episode(&id).unwrap();
        assert_eq!(g.full_turns(&id), vec![x.id]);
        assert_eq!(ep.segments(), &[Segment::Own(vec![x.occurrence()])]);
        assert!(ep.origin.is_none(), "no graft point → no origin");
    }

    #[test]
    fn re_root_rejects_a_bad_source_or_index_without_mutating() {
        let (mut g, root_id, ..) = abc_root();
        let before = g.episode_ids().len();
        let id = EpisodeId::from_seed(b"nope");
        assert_eq!(
            g.re_root(root_id, 4, vec![turn("x")], cfg(), id),
            None,
            "from past the source's length"
        );
        assert_eq!(
            g.re_root(
                EpisodeId::from_seed(b"ghost"),
                0,
                vec![turn("x")],
                cfg(),
                id
            ),
            None,
            "unknown source"
        );
        assert_eq!(
            g.episode_ids().len(),
            before,
            "no episode inserted on failure"
        );
    }

    #[test]
    fn splice_expresses_a_middle_squash() {
        // The case the old prefix-only model could not say: replace the MIDDLE
        // turn b with a summary, keeping [a] and [c] shared on either side.
        let (mut g, root_id, a, b, c) = abc_root();
        let summary = turn("summary");
        let id = EpisodeId::from_seed(b"squash");
        g.insert_content(summary.clone());
        g.splice(
            id,
            cfg(),
            vec![
                Segment::Slice {
                    episode: root_id,
                    start: 0,
                    end: 1,
                },
                Segment::Own(vec![summary.occurrence()]),
                Segment::Slice {
                    episode: root_id,
                    start: 2,
                    end: 3,
                },
            ],
            Some(Origin {
                episode: root_id,
                turn: b.id,
                index: Some(1),
                kind: EdgeKind::Branch,
            }),
        );
        assert_eq!(
            g.full_turns(&id),
            vec![a.id, summary.id, c.id],
            "[a] · summary · [c] — a and c shared, b replaced"
        );
        assert_eq!(g.episode(&id).unwrap().own_turns(), vec![summary.id]);
    }

    #[test]
    fn project_of_a_plain_root_is_one_slice_to_itself() {
        // A root owns all its turns, so a projection is a single slice pointing at
        // itself over the requested range (adjacent own turns coalesce).
        let (g, root_id, ..) = abc_root(); // root = [a, b, c]
        assert_eq!(
            g.project(&root_id, 0, 3),
            vec![Segment::Slice {
                episode: root_id,
                start: 0,
                end: 3,
            }]
        );
        assert_eq!(
            g.project(&root_id, 1, 3),
            vec![Segment::Slice {
                episode: root_id,
                start: 1,
                end: 3,
            }]
        );
    }

    #[test]
    fn project_references_the_owner_not_a_pass_through_intermediate() {
        // The load-bearing property: b1 borrows root's [a, b] (b1 owns nothing).
        // Projecting b1's prefix must reference root (the true OWNER), never b1 —
        // so feeding a projection back into `splice` keeps the graph shallow and
        // keeps the delete-liveness set to real data owners.
        let (mut g, root_id, ..) = abc_root();
        let b1 = EpisodeId::from_seed(b"b1");
        g.branch_prefix(root_id, 2, b1); // b1 = [Slice(root, 0..2)], owns nothing
        assert_eq!(
            g.project(&b1, 0, 2),
            vec![Segment::Slice {
                episode: root_id,
                start: 0,
                end: 2,
            }],
            "projects to root, not to the pass-through b1"
        );
    }

    #[test]
    fn project_splits_a_continued_branch_by_owner() {
        // b1 shares root's [a, b] then diverges with its own d ([a, b, d]).
        // Projecting the whole thing yields two slices: root's [a, b] and b1's own
        // d — whose owner-index is its DERIVED index (2), not 0.
        let (mut g, root_id, _a, _b, _c) = abc_root();
        let d = turn("d");
        let b1 = EpisodeId::from_seed(b"b1");
        g.branch_prefix(root_id, 2, b1);
        g.insert_content(d.clone());
        g.episode_mut(&b1).unwrap().append(d.occurrence()); // b1 = [a, b, d]
        assert_eq!(
            g.project(&b1, 0, 3),
            vec![
                Segment::Slice {
                    episode: root_id,
                    start: 0,
                    end: 2,
                },
                Segment::Slice {
                    episode: b1,
                    start: 2,
                    end: 3,
                },
            ]
        );
    }

    #[test]
    fn project_keeps_non_consecutive_same_owner_ranges_separate() {
        // An episode whose derived sequence puts root's turn 0 and turn 2 ADJACENT
        // (skipping turn 1). Coalescing them into a single {root, 0..3} would wrongly
        // re-include the skipped turn, so the guard must keep two slices.
        let (mut g, root_id, a, _b, c) = abc_root();
        let gap = EpisodeId::from_seed(b"gap");
        g.splice(
            gap,
            cfg(),
            vec![
                Segment::Slice {
                    episode: root_id,
                    start: 0,
                    end: 1,
                },
                Segment::Slice {
                    episode: root_id,
                    start: 2,
                    end: 3,
                },
            ],
            None,
        );
        assert_eq!(g.full_turns(&gap), vec![a.id, c.id]);
        assert_eq!(
            g.project(&gap, 0, 2),
            vec![
                Segment::Slice {
                    episode: root_id,
                    start: 0,
                    end: 1,
                },
                Segment::Slice {
                    episode: root_id,
                    start: 2,
                    end: 3,
                },
            ],
            "non-consecutive owner indices must not coalesce"
        );
    }

    #[test]
    fn project_composes_a_copy_on_write_middle_edit() {
        // The App's clone-edit path, expressed purely via `project`: a full-share
        // clone of root, then edit the middle turn keeping both flanks — and the
        // original is untouched, the flanks reference the owner (root).
        let (mut g, root_id, a, b, c) = abc_root(); // root = [a, b, c]
        let clone_id = EpisodeId::from_seed(b"clone");
        g.splice(
            clone_id,
            cfg(),
            g.project(&root_id, 0, 3), // full-share == [Slice(root, 0..3)]
            None,
        );
        assert_eq!(g.full_turns(&clone_id), vec![a.id, b.id, c.id]);

        // COW edit at index 1: project(0,1) ++ Own(edited) ++ project(2,3).
        let edited = turn("edited");
        let mut segments = g.project(&clone_id, 0, 1);
        segments.push(Segment::Own(vec![edited.occurrence()]));
        segments.extend(g.project(&clone_id, 2, 3));
        let edit_id = EpisodeId::from_seed(b"edit");
        g.insert_content(edited.clone());
        g.splice(edit_id, cfg(), segments, None);

        assert_eq!(
            g.full_turns(&edit_id),
            vec![a.id, edited.id, c.id],
            "middle replaced, both flanks kept (COW)"
        );
        // Flanks reference root (the owner), and only `edited` is new data.
        assert_eq!(g.episode(&edit_id).unwrap().own_turns(), vec![edited.id]);
        assert_eq!(
            g.full_turns(&root_id),
            vec![a.id, b.id, c.id],
            "the original is untouched"
        );
    }

    #[test]
    fn project_clamps_out_of_range_and_empties_a_reversed_range() {
        let (g, root_id, ..) = abc_root();
        assert_eq!(
            g.project(&root_id, 0, 99),
            vec![Segment::Slice {
                episode: root_id,
                start: 0,
                end: 3,
            }],
            "end clamps to the derived length"
        );
        assert!(
            g.project(&root_id, 2, 1).is_empty(),
            "reversed range → empty"
        );
        assert!(
            g.project(&EpisodeId::from_seed(b"nope"), 0, 3).is_empty(),
            "unknown episode → empty"
        );
    }

    #[test]
    fn resolve_handles_a_diamond_without_double_counting_state() {
        // An episode that resolves `root` via two independent slices (a diamond in
        // the reference DAG) must resolve each independently — no shared mutable
        // state, no cycle false-positive.
        let (mut g, root_id, a, b, _c) = abc_root();
        let d = turn("d");
        let mid = EpisodeId::from_seed(b"mid");
        g.branch_prefix(root_id, 2, mid); // Slice(root, 0..2) = [a, b]
        g.insert_content(d.clone());
        g.episode_mut(&mid).unwrap().append(d.occurrence()); // mid = [a, b, d]

        // top slices root directly AND slices mid (which slices root again).
        let top = EpisodeId::from_seed(b"top");
        g.splice(
            top,
            cfg(),
            vec![
                Segment::Slice {
                    episode: root_id,
                    start: 0,
                    end: 1,
                },
                Segment::Slice {
                    episode: mid,
                    start: 0,
                    end: 3,
                },
            ],
            None,
        );
        assert_eq!(
            g.full_turns(&top),
            vec![a.id, a.id, b.id, d.id],
            "[a] then mid's [a, b, d] — root resolved twice, independently"
        );
    }

    #[test]
    fn resolve_breaks_a_cyclic_slice_reference() {
        // Corrupt data: two episodes slice each other. Resolution must terminate
        // (empty) rather than recurse forever.
        let mut g = Graph::new();
        let (e1, e2) = (EpisodeId::from_seed(b"e1"), EpisodeId::from_seed(b"e2"));
        g.insert_episode(Episode::from_segments(
            e1,
            cfg(),
            vec![Segment::Slice {
                episode: e2,
                start: 0,
                end: 1,
            }],
            None,
        ));
        g.insert_episode(Episode::from_segments(
            e2,
            cfg(),
            vec![Segment::Slice {
                episode: e1,
                start: 0,
                end: 1,
            }],
            None,
        ));
        assert!(
            g.full_turns(&e1).is_empty(),
            "cycle resolves empty, no hang"
        );
    }

    /// **An origin cycle ends the chain instead of hanging it** — corrupt
    /// data: two episodes each cut from the other. `depth`, `child_depth`
    /// and the TUI's walk over [`Graph::origin_chain`] all stop there (review
    /// stop 5, B 1: the core had no test of it).
    #[test]
    fn an_origin_cycle_ends_the_chain() {
        let mut g = Graph::new();
        let (e1, e2) = (EpisodeId::from_seed(b"e1"), EpisodeId::from_seed(b"e2"));
        let x = turn("x");
        let from = |episode| Origin {
            episode,
            turn: x.id,
            index: Some(0),
            kind: EdgeKind::Branch,
        };
        g.insert_episode(Episode::from_segments(
            e1,
            cfg(),
            Vec::new(),
            Some(from(e2)),
        ));
        g.insert_episode(Episode::from_segments(
            e2,
            cfg(),
            Vec::new(),
            Some(from(e1)),
        ));
        let chain: Vec<EpisodeId> = g.origin_chain(&e1).iter().map(|o| o.episode).collect();
        assert_eq!(chain, [e2], "e2, then e1 again: stopped");
        assert_eq!(g.depth(&e1), 1);
    }

    #[test]
    fn full_turns_derives_through_a_multi_level_chain() {
        // root [a,b,c]; b1 branches sharing [a,b] then diverges with d ([a,b,d]);
        // b2 continues b1 (shares all of [a,b,d]) then diverges with f
        // ([a,b,d,f]). resolve must walk both levels of slices.
        let (mut g, root_id, a, b, _c) = abc_root();
        let (d, f) = (turn("d"), turn("f"));

        let b1 = EpisodeId::from_seed(b"b1");
        g.branch_prefix(root_id, 2, b1); // Slice(root, 0..2)
        g.insert_content(d.clone());
        g.episode_mut(&b1).unwrap().append(d.occurrence()); // b1 = [a,b,d]

        // b2 continues b1: shares all of b1's derived [a,b,d] then adds f.
        let b2 = EpisodeId::from_seed(b"b2");
        let mut b2_ep = Episode::branch(
            b2,
            cfg(),
            Origin {
                episode: b1,
                turn: d.id,
                index: Some(2),
                kind: EdgeKind::Branch,
            },
            3,
        );
        b2_ep.append(f.occurrence());
        g.insert_content(f.clone());
        g.insert_episode(b2_ep);

        assert_eq!(g.full_turns(&root_id), vec![a.id, b.id, _c.id]);
        assert_eq!(g.full_turns(&b1), vec![a.id, b.id, d.id]);
        assert_eq!(g.full_turns(&b2), vec![a.id, b.id, d.id, f.id]);
        assert_eq!(g.head(&b2), Some(f.id));

        assert_eq!(g.episodes_with_turn(&a.id), 3, "a is shared by all three");
        assert_eq!(g.episodes_with_turn(&_c.id), 1, "c only on root");
        assert_eq!(g.episodes_with_turn(&d.id), 2, "d on b1 and b2");
    }

    #[test]
    fn full_turns_of_a_fresh_fork_is_just_the_shared_prefix() {
        // A just-forked branch owns nothing; its full history is the derived
        // prefix, and its head is the last shared turn (no own head yet).
        let (mut g, root_id, a, b, _c) = abc_root();
        let fork = EpisodeId::from_seed(b"fork");
        g.branch_prefix(root_id, 2, fork); // shares [a,b], no own turns
        assert!(g.episode(&fork).unwrap().own_is_empty());
        assert_eq!(g.full_turns(&fork), vec![a.id, b.id]);
        assert_eq!(g.head(&fork), Some(b.id), "tip is the last shared turn");
    }
}
