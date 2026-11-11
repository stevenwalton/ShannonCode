//! An **Episode**: an ordered sequence of content-addressed Turns, plus the
//! config the next turn runs under and a pointer to where it forked from.
//!
//! Per design.md §1.1 the containment is `Message ≤ Turn ≤ Episode`: an Episode
//! is "(mostly) an agent's context/history." Unlike a [`TurnId`] — which is
//! content-addressed and *immutable* — an Episode is **mutable**: turns are
//! appended and its head moves. So its identity cannot be its content; an
//! [`EpisodeId`] is assigned once at birth (from an immutable birth-seed) and
//! never changes. The git analogy is exact: a Turn is a commit (hashed by its
//! bytes), an Episode is a branch (a named, moving head over a sequence of those
//! commits).
//!
//! Episodes link into a DAG through the [`Origin`] on each non-root episode:
//! the parent Episode, the specific Turn it forked at, and an [`EdgeKind`] saying
//! *why* (a user `Branch`, or a `ToolCall` sub-agent). The set of episodes plus
//! those parent pointers is the queryable graph walked by [`super::graph::Graph`].

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::graph::turn::{Occurrence, TurnConfig, TurnId};
use crate::graph::{decode_hex32, encode_hex32};

/// How many hex chars a short id shows — git's short-hash convention.
///
/// One constant so every surface that abbreviates an id agrees: the two
/// `short_hex` accessors, the `:` command line's completion, and the log
/// gutter. Four independent `8`s is how the UI ended up with divergent codings
/// once already.
pub const SHORT_ID_LEN: usize = 8;

/// **The one rule for abbreviating an id**, over a string rather than a type.
///
/// Both [`EpisodeId::short_hex`] and [`TurnId::short_hex`] call it, and so does
/// the log formatter, which only ever sees a field's rendered text and has no
/// id type to ask. Anything already shorter is returned whole.
///
/// Char-indexed rather than sliced at a byte offset: a real id is ASCII hex so
/// the two agree, but this is also reachable from the log formatter with
/// whatever a call site put in an `episode` field, and slicing a byte offset
/// that is not a char boundary panics
/// (`[[feedback_char_boundary_slicing]]`).
pub fn short_hex(id: &str) -> &str {
    match id.char_indices().nth(SHORT_ID_LEN) {
        Some((end, _)) => &id[..end],
        None => id,
    }
}

/// Stable handle for an [`Episode`]. Assigned at birth and never changed (an
/// Episode is mutable, so it can't be addressed by its content the way a
/// [`TurnId`] is). 256-bit, `Copy` so it's cheap to use as a key.
///
/// Minted by hashing an immutable **birth-seed** — for tests, a fixed seed gives
/// a fixed id; production callers pass something unique (time + nonce, or a
/// name). Where that production uniqueness comes from is a persistence-layer
/// concern (P2); this in-memory model only needs the type and a deterministic
/// constructor. Serde form is the **hex string** (like [`TurnId`]) so episode
/// files are named and `grep`-able by id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EpisodeId([u8; 32]);

impl EpisodeId {
    /// **The default episode's id, a fixed value** — what a store written
    /// before `:new` keyed its one session by, and what a launch falls back
    /// to when a store has no `HEAD` (`Conversation::default_episode_id`). It
    /// was minted from the seed `default` by the derivation of its day, and
    /// must not move when the derivation does (the tag changed 11-08): a
    /// value, so it cannot.
    pub const DEFAULT_EPISODE: Self = Self([
        0xfc, 0x79, 0x18, 0xeb, 0x04, 0xc9, 0x3f, 0xd6, //
        0xeb, 0x19, 0xba, 0x2c, 0xbf, 0x7b, 0xb5, 0xc8, //
        0xd3, 0x87, 0xbe, 0x9c, 0x8b, 0xce, 0x63, 0xcc, //
        0x41, 0x18, 0xcb, 0x8a, 0x04, 0x55, 0x29, 0x44,
    ]);

    /// Mint an id from an immutable birth-seed. Domain-separated from the Turn
    /// hash space by a fixed prefix so the two id spaces can never collide even
    /// if a seed happened to equal some turn's bytes.
    pub fn from_seed(seed: &[u8]) -> Self {
        let mut h = blake3::Hasher::new();
        // An id minted here is stored and never derived again — the one that
        // was, the default, is a value now (`DEFAULT_EPISODE`) — so the tag
        // moves only the ids minted after it (Steven, 11-08: Shannon's
        // spelling; `the_episode_id_derivation_is_pinned`).
        h.update(b"shannon.episode\0");
        h.update(seed);
        Self(*h.finalize().as_bytes())
    }

    /// The id as 64 lowercase hex chars — for display, refs, and `cat`-able logs.
    pub fn to_hex(&self) -> String {
        encode_hex32(&self.0)
    }

    /// The git-short form, for notices / labels / the inspector.
    ///
    /// Delegates to [`short_hex`] rather than slicing, so this and every other
    /// reader of the short form share one rule. It used to slice `[..8]` here
    /// and call itself "the one canonical short-id"; that was already untrue
    /// (`cmdline.rs` carried its own `SHORT_ID = 8`) and a third copy then
    /// appeared in the log formatter. A comment claiming completeness is the
    /// highest-risk artifact in this codebase — the fix is to make the claim
    /// true by construction (`[[feedback_rules_rot_by_retyping]]`).
    pub fn short_hex(&self) -> String {
        short_hex(&self.to_hex()).to_string()
    }

    /// Parse an id from 64 hex chars (either case); `None` if malformed. Used to
    /// read an episode id back from a ref file or JSON.
    pub fn from_hex(s: &str) -> Option<Self> {
        decode_hex32(s).map(Self)
    }

    /// The raw 32 id bytes (used for deterministic ordering in graph traversal).
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Serialize as the hex string so episode files / refs are inspectable.
impl Serialize for EpisodeId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for EpisodeId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let hex = String::deserialize(deserializer)?;
        Self::from_hex(&hex)
            .ok_or_else(|| serde::de::Error::custom("EpisodeId: expected 64 hex chars"))
    }
}

/// Why a child Episode exists relative to its parent — the parent-edge kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// A user-initiated alternate line of history forked from a committed turn —
    /// the edit-a-past-turn → branch case (design.md §1.2). Diverges *mid*-history.
    Branch,
    /// A **clone**: a fork at the parent's *head* that shares its whole history and
    /// continues in parallel (`:clone`, design.md §1.2). Structurally a fork like
    /// [`Branch`](Self::Branch), but a distinct intent — a sibling session, not an
    /// edit — so the viewer can present it differently (it reads as "a copy of that
    /// session," not "an edit of it").
    Clone,
    /// A sub-agent episode spawned by a tool-call inside the parent turn
    /// (design.md §1.x). The variant exists now so the model and the viewer
    /// admit it; live sub-agent creation is earned in P6.
    ToolCall,
    /// A **merge** (or squash / prune / pick): an episode *composed* from spans of
    /// one or more sources (design.md §1.7, the `MergeStrategy` engine). Reads as its
    /// own session — the originals are always preserved. Origin is single-parent
    /// (presentation: the primary base), but a merge's true provenance is N-ary and
    /// lives in its [`Segment`]s (which may slice several source episodes).
    Merge,
}

impl EdgeKind {
    /// The user-facing word for this origin kind — `branch` / `clone` /
    /// `child` / `merge`. **`ToolCall` reads as `child`** (the `:child`
    /// spawn's user-facing name). One place, so the tree label, the pane
    /// branch-indicator, and the inspector can't drift (and a 5th variant is
    /// a compile error here, not four silently-stale match arms).
    pub fn label(self) -> &'static str {
        match self {
            EdgeKind::Branch => "branch",
            EdgeKind::Clone => "clone",
            EdgeKind::ToolCall => "child",
            EdgeKind::Merge => "merge",
        }
    }
}

/// One contiguous piece of an Episode's derived turn sequence — the unit of
/// structural sharing (design.md §1.1). An Episode's history is a **path** built
/// from these pieces: some turns it contributed itself, some borrowed *by
/// reference* (never copied) from another episode. This is a piece table over the
/// shared Turn set, and it is what lets "share the data unless it is changed" hold
/// in *every* direction — prefix (branch), suffix (re-root/clone), or middle
/// (squash) — rather than only for prefixes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Segment {
    /// Turns this episode contributed itself — the **only** place new data lives.
    /// (A branch's divergence, a re-root's new root, a squash's summary.) So an
    /// episode's storage cost is exactly its changed turns.
    ///
    /// Each entry is an [`Occurrence`]: the content's address plus the facts of
    /// producing it HERE. Content may already exist elsewhere (identical turns
    /// share one id and are stored once); the occurrence never does.
    Own(Vec<Occurrence>),
    /// A half-open slice `[start, end)` of another episode's *derived* sequence,
    /// shared by reference. Because ids are content-addressed and
    /// position-independent, the slice denotes the same turns viewed from either
    /// side, and nothing is duplicated — only the (episode, range) record is stored.
    Slice {
        episode: EpisodeId,
        start: usize,
        end: usize,
    },
}

/// Where an Episode was **born** — presentation metadata: the source episode, the
/// turn it forked/grafted at, and why ([`EdgeKind`]). A forest root (a top-level
/// conversation) has `origin == None`.
///
/// *History derivation* ([`Graph::full_turns`]/`resolve`) never reads this — it
/// resolves [`Segment`]s only. But the **tree-walk shape** does depend on it: `kind`
/// decides whether an episode is a top-level session ([`is_session_root`]) or hangs
/// under the turn it forked from, and `(episode, turn)` is where the viewer attaches
/// it. So Origin is presentation for the *content* of history, structure for the
/// *shape* of the forest — the two just don't tangle in one field the way the
/// earlier `ParentLink` did.
///
/// With content-addressing the same [`TurnId`] recurs across episodes, so `turn` is
/// only unambiguous together with `episode`.
///
/// [`Graph::full_turns`]: super::graph::Graph::full_turns
/// [`is_session_root`]: Episode::is_session_root
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Origin {
    pub episode: EpisodeId,
    pub turn: TurnId,
    /// The attach turn's **occurrence index** in `episode`'s derived sequence.
    /// Content-addressed ids recur, so `turn` alone is ambiguous under duplicate
    /// content — without this a child rendered under EVERY occurrence of a
    /// repeated turn (the third strike of [[content-addressed-ids]]). `None` is
    /// a legacy record (pre-index), resolved deterministically as the id's
    /// FIRST occurrence; every new origin stores the real index.
    #[serde(default)]
    pub index: Option<usize>,
    pub kind: EdgeKind,
}

/// What a label refuses of all its text: a control, or what
/// [`misleads`](crate::text::misleads).
fn unplain(c: char) -> bool {
    c.is_control() || crate::text::misleads(c)
}

/// **What a person calls a session** — a name to type and read instead of
/// its id (`:rename`), and a line saying what it is (`:description`)
/// (Steven, 10-14: "display metadata, not destroying the hash"). The id
/// stays the identity; this is presentation, like [`Origin`], and derivation
/// never reads it. Nothing inherits it — a clone, a branch or a child starts
/// unnamed, so a name stays one session's. What a name may be is the
/// reader's rule (the TUI's, beside the other readings of a session token),
/// not the store's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Label {
    /// The longest a name may be, in characters — a border shows it.
    pub const NAME_MOST: usize = 48;
    /// The longest a description may be, in characters — one line of an
    /// inspector, not a document.
    pub const DESCRIPTION_MOST: usize = 200;

    /// Nothing said — what a header with no `label` reads as, and what is
    /// not written.
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.description.is_none()
    }

    /// **Could `text` be one line a person reads as it is written?** No
    /// control character, nothing that reorders or hides what it says
    /// ([`misleads`](crate::text::misleads)), and no more than `most`
    /// characters. Every language's letters, marks and joiners are plain: a
    /// Persian description's non-joiner was refused as "not plain text"
    /// (review stop 9, E2).
    pub fn plain(text: &str, most: usize) -> bool {
        text.chars().count() <= most && !text.chars().any(unplain)
    }

    /// **A label as read back from a file**, which a person may have edited
    /// by hand: a name that could not have been given — empty, too long,
    /// spaced, not [`plain`](Self::plain), holding a character that draws as
    /// nothing — is dropped (the session reads as unnamed, its id intact),
    /// and a description loses what is not plain and what is past its
    /// length. The front end checks names further when they are given; this
    /// is the shape every reader may rely on.
    pub fn read_back(mut self) -> Label {
        let named = |n: &String| {
            !n.is_empty()
                // A name is a token you type back: one drawing as nothing
                // would make `auth\u{200B}fix` and `authfix` read alike.
                && !(n.chars()).any(|c| c.is_whitespace() || crate::text::draws_as_nothing(c))
                && Self::plain(n, Self::NAME_MOST)
        };
        self.name = self.name.filter(named);
        self.description = self.description.and_then(|d| {
            let kept: String = d
                .chars()
                .filter(|&c| !unplain(c))
                .take(Self::DESCRIPTION_MOST)
                .collect();
            (!kept.trim().is_empty()).then_some(kept)
        });
        self
    }

    /// **Session `id`, named in a sentence** — `auth-fix 1a2b3c4d`, or the
    /// short id alone when it has no name: the name a person reads, and the
    /// id the graph rows and the log show beside it. The one spelling for a
    /// front end's notices and confirmations, so what they say matches what
    /// its graph lists.
    pub fn tag(&self, id: EpisodeId) -> String {
        match &self.name {
            Some(name) => format!("{name} {}", id.short_hex()),
            None => id.short_hex(),
        }
    }
}

/// **The Claude Code conversation a session continues** — recorded when it
/// was brought in from one (`:import`, `<Enter>` on a Claude Code row in
/// `:graph`), so a front end can hang it under that conversation and tell a
/// session that continues one from a session born in Shannon (Steven, 10-16).
/// It was recorded nowhere: an import was a root like any other.
///
/// On the imported session alone. What descends from it — a clone, a branch,
/// a squash — continues it too, and is found by its lineage, not by a copy of
/// this; a copy would be one more thing to keep true. Presentation, like
/// [`Origin`]: derivation never reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inherited {
    /// Claude Code's session id — its transcript's file name.
    pub session: String,
    /// How many of its turns came in: where the two part.
    pub turns: usize,
    /// **Which of its turns the last to come in was**, as Claude Code names
    /// the entry that opened it — where `:graph` hangs the session under the
    /// conversation, whatever Claude Code has written since (a count alone
    /// cannot say where: an import starts at the compaction that was latest
    /// then). `None` for a record from before it was kept, or a turn whose
    /// prompt was not read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// **The entry that opened each turn that came in**, in their order —
    /// where each hangs under the conversation, and a branch cut from it: an
    /// import from the latest compaction holds what that compaction kept
    /// AFTER its summary, which the conversation as `:graph` reads it holds
    /// before, so no count back from `at` lines the two up (review stop 4,
    /// A 1). Empty for a record from before it was kept, placed by that
    /// count; `None` for a turn whose prompt was not read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opened: Vec<Option<String>>,
}

/// A session as a **path through the shared Turn set** (design.md §1.1), stored as
/// a **piece table**: an ordered list of [`Segment`]s — some turns it contributed
/// itself ([`Segment::Own`]), some borrowed by reference from other episodes
/// ([`Segment::Slice`]). The full history (the logical "ordered sequence of Turns")
/// is *derived* by resolving those segments — `Graph::full_turns`. Only `Own`
/// turns are new data, so every operation costs exactly its changed turns; a branch
/// is `[Slice(parent, 0..k), Own(..)]`, a clone/re-root is `[Own(new_root),
/// Slice(source, k..n)]`, a squash is `[Slice(..), Own(summary), Slice(..)]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Episode {
    /// Stable birth-assigned handle.
    pub id: EpisodeId,
    /// The pieces of this episode's derived sequence, in order. Private to protect
    /// the **append-only** invariant: segments only grow and existing pieces never
    /// move, so the indices a descendant's [`Segment::Slice`] pins into this
    /// episode stay valid forever. Read via [`segments`](Self::segments); derive the
    /// turns via [`Graph::full_turns`].
    segments: Vec<Segment>,
    /// The active config the next appended turn will be sealed under. A fork can
    /// override it (e.g. switch models mid-history).
    pub config: TurnConfig,
    /// Where this episode was born (viewer attachment + edge label), or `None` for a
    /// forest root. Presentation only — derivation ignores it.
    pub origin: Option<Origin>,
    /// Its name and description, if a person gave them ([`Label`]).
    pub label: Label,
    /// The Claude Code conversation it was brought in from, if it was
    /// ([`Inherited`]).
    pub inherited: Option<Inherited>,
    /// **Its marks**, yours and the agent's ([`Mark`](super::Mark)), kept
    /// in its header. `None`: none was ever saved — the header carries no
    /// key, its bytes as they always were; `Some`, empty, once all are gone,
    /// so a header that no longer has them says so rather than leaving the
    /// last one's standing. Presentation, like [`Origin`]: derivation never
    /// reads it. **Indices, not occurrences**: a surgery that removes turns
    /// before a mark (`with_segments`) leaves it naming the turn after —
    /// docketed with the surgeries (NQ1).
    pub marks: Option<Vec<super::Mark>>,
    /// **Saved entries this build does not read**, as they were — another
    /// build's shape, a hand edit, a name no key reaches: written back with
    /// [`marks`](Self::marks) at every header, so a save here never erases
    /// them (review stop 14, B F4). Read and bounded by the store
    /// (`mark::read_saved`).
    pub unread_marks: Vec<serde_json::Value>,
}

/// Append an occurrence to a segment list — extending the trailing
/// [`Segment::Own`] piece, or opening a new one if the last piece is a
/// [`Segment::Slice`]. The one shape shared by [`Episode::append`], the graph's
/// own rebuilds and the stores replaying a turn record, so the "coalesce
/// consecutive appends into one Own piece" invariant lives in a single place
/// rather than being re-implemented per store backend.
pub(crate) fn push_own(segments: &mut Vec<Segment>, occurrence: Occurrence) {
    match segments.last_mut() {
        Some(Segment::Own(ts)) => ts.push(occurrence),
        _ => segments.push(Segment::Own(vec![occurrence])),
    }
}

impl Episode {
    /// A new forest-root episode (no origin) with no turns yet.
    pub fn root(id: EpisodeId, config: TurnConfig) -> Self {
        Self {
            id,
            segments: Vec::new(),
            config,
            origin: None,
            label: Label::default(),
            inherited: None,
            marks: None,
            unread_marks: Vec::new(),
        }
    }

    /// An episode built from explicit [`Segment`]s — the general constructor the
    /// [`splice`](super::graph::Graph::splice) op stands on (re-root, squash, graft).
    pub fn from_segments(
        id: EpisodeId,
        config: TurnConfig,
        segments: Vec<Segment>,
        origin: Option<Origin>,
    ) -> Self {
        Self {
            id,
            segments,
            config,
            origin,
            label: Label::default(),
            inherited: None,
            marks: None,
            unread_marks: Vec::new(),
        }
    }

    /// **This episode, its pieces replaced** — a rewrite of the session
    /// itself (`Graph::remove_turns`, `Graph::detach`), which a store writes
    /// whole. Everything else it carries stays: rebuilt through
    /// [`from_segments`](Self::from_segments), a turn delete dropped its
    /// name and its import record (review stop 2, A F1) — and would have
    /// dropped any field added after them.
    pub(crate) fn with_segments(self, segments: Vec<Segment>) -> Self {
        Self { segments, ..self }
    }

    /// A new **branch**: shares the first `prefix_len` turns of `origin.episode`
    /// (a single leading [`Segment::Slice`]) and starts with no own turns yet — the
    /// caller appends whatever diverges. The prefix-sharing special case of a
    /// splice. `prefix_len == 0` shares nothing (a branch off the very start).
    pub fn branch(id: EpisodeId, config: TurnConfig, origin: Origin, prefix_len: usize) -> Self {
        let segments = if prefix_len == 0 {
            Vec::new()
        } else {
            vec![Segment::Slice {
                episode: origin.episode,
                start: 0,
                end: prefix_len,
            }]
        };
        Self::from_segments(id, config, segments, Some(origin))
    }

    /// **What a person calls this session where there is room for one word**
    /// — its name if it has one, else its short id. Either is a token every
    /// session verb reads, so what a border shows is what you can type.
    pub fn handle(&self) -> String {
        self.label
            .name
            .clone()
            .unwrap_or_else(|| self.id.short_hex())
    }

    /// **Where this session came from, as it reads beside its id** —
    /// `branch ← <turn>`, `merge ← <turn>` …: where it forked; `at <turn>`
    /// for a branch with no turn of its own yet — a checkout looked at and
    /// not continued, where "branch" said one had been made (S21-Q8;
    /// Steven, 11-01: "I lean towards a but don't like the 'checkout'
    /// wording" — git's own "HEAD detached at"); `clone of <session>` for a
    /// clone (`:clone`, `:fork`, `:vfork`), which shares every turn: its
    /// arrow named its source's FIRST turn, read as where it forked and
    /// drawn as a checkout of turn 1 is (review stop 15, F L8). `None` for a
    /// root. The one wording of the tree's row, a transcript's title and the
    /// inspector's origin, as [`EdgeKind::label`] is the one word.
    pub fn origin_clause(&self) -> Option<String> {
        let origin = self.origin?;
        let turn = origin.turn.short_hex();
        Some(match (origin.kind, self.own_len()) {
            (EdgeKind::Branch, 0) => format!("at {turn}"),
            (EdgeKind::Clone, _) => format!("clone of {}", origin.episode.short_hex()),
            (kind, _) => format!("{} ← {turn}", kind.label()),
        })
    }

    /// The pieces composing this episode's derived sequence, in order.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// This episode's own (self-contributed) turns, oldest first — the
    /// concatenation of its [`Segment::Own`] pieces, *not* the borrowed/shared
    /// turns. These are exactly the turns this episode must persist (a `Slice` is
    /// derived, never re-stored). For the full derived history use
    /// `Graph::full_turns`.
    pub fn own_turns(&self) -> Vec<TurnId> {
        self.own_occurrences().map(|o| o.turn).collect()
    }

    /// This episode's own occurrences, oldest first — [`own_turns`](Self::own_turns)
    /// with the facts of each production.
    pub fn own_occurrences(&self) -> impl Iterator<Item = &Occurrence> {
        self.segments.iter().flat_map(|s| match s {
            Segment::Own(ts) => ts.as_slice(),
            Segment::Slice { .. } => &[],
        })
    }

    /// The tip of this episode's *own* turns (the last turn it contributed), or
    /// `None` if it has added none yet. Not necessarily the session's visible tip:
    /// a re-root's tip is the last of its shared suffix — see `Graph::head`.
    pub fn own_head(&self) -> Option<TurnId> {
        self.own_occurrences().last().map(|o| o.turn)
    }

    /// Append an occurrence to this episode's own turns — extending its trailing
    /// [`Segment::Own`] piece, or opening a new one if the last piece is a `Slice`.
    /// The occurrence comes from [`Graph::admit`](super::graph::Graph::admit),
    /// which puts the sealed turn's content where every occurrence can read it.
    pub fn append(&mut self, occurrence: Occurrence) {
        push_own(&mut self.segments, occurrence);
    }

    /// How many *own* (self-contributed) turns this episode records — not counting
    /// borrowed/shared turns.
    pub fn own_len(&self) -> usize {
        self.own_occurrences().count()
    }

    /// Whether this episode is a top-level **session** — a true forest root with
    /// no origin. Clones and merges used to count as roots too, but an episode
    /// with an origin is *also* listed under its origin turn by `Graph::children`,
    /// so root-ing them drew the same session in TWO places (a dogfooding bug: a
    /// merge appeared both nested and as a second root). One node, one place:
    /// rooted iff there is nothing to hang from. What a nested clone/merge
    /// *displays* is a separate concern — [`shows_full_history`](Self::shows_full_history).
    pub fn is_session_root(&self) -> bool {
        self.origin.is_none()
    }

    /// Whether the tree should draw this episode's **full derived history**
    /// (shared turns included, grayed) rather than only the turns it owns. A
    /// clone or a merge owns little or nothing — its derived sequence IS its
    /// content — while a branch's shared prefix is already drawn once, under the
    /// ancestor that owns it. Display policy, but it lives beside
    /// [`is_session_root`](Self::is_session_root) because the two were once conflated and must be read
    /// together.
    pub fn shows_full_history(&self) -> bool {
        self.origin
            .is_none_or(|o| matches!(o.kind, EdgeKind::Clone | EdgeKind::Merge))
    }

    /// Whether the episode has contributed no turns of its own yet (a fresh root,
    /// or a just-forked branch that hasn't diverged).
    pub fn own_is_empty(&self) -> bool {
        !self
            .segments
            .iter()
            .any(|s| matches!(s, Segment::Own(ts) if !ts.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TurnConfig {
        TurnConfig {
            model: "claude-haiku".into(),
            max_tokens: None,
            effort: None,
        }
    }

    fn turn_id(seed: &str) -> TurnId {
        // A stand-in TurnId; content-address correctness is turn.rs's job.
        use crate::message::Message;
        TurnId::compute(&[Message::user(seed)], &cfg())
    }

    /// A stand-in occurrence of `seed`'s turn.
    fn occ(seed: &str) -> Occurrence {
        use crate::message::Message;
        crate::graph::Turn::seal(vec![Message::user(seed)], cfg(), std::time::UNIX_EPOCH)
            .occurrence()
    }

    /// **A label read back from a file is one a person could have given** —
    /// a name that is spaced, too long, empty or carries invisible formatting
    /// is dropped (the session reads as unnamed, its id intact), and a
    /// description loses its control and bidi characters and what is past
    /// its bound (review of 34033e8, Reviewer B: loaded labels were trusted).
    #[test]
    fn a_label_read_back_is_one_a_person_could_have_given() {
        let back = |name: &str, about: &str| {
            Label {
                name: Some(name.to_string()),
                description: Some(about.to_string()),
            }
            .read_back()
        };
        assert_eq!(back("auth-fix", "fine").name.as_deref(), Some("auth-fix"));
        assert_eq!(back("인증-수정", "fine").name.as_deref(), Some("인증-수정"));
        for bad in [
            "two words",
            "",
            "x\u{202e}y",
            "tab\there",
            "auth\u{200b}fix",
            &"n".repeat(49),
        ] {
            assert_eq!(back(bad, "fine").name, None, "{bad:?}");
        }
        let about = back("ok", "a\u{7}b\u{202e}c\nd\u{e0041}").description;
        assert_eq!(about.as_deref(), Some("abcd"), "control, bidi, tag dropped");
        // Every language's text as written (review stop 9, E2): Persian's
        // non-joiner, Thai's zero-width space, Korean, an accent combining.
        for text in [
            "می\u{200C}خواهم",
            "สวัสดี\u{200B}ครับ",
            "인증 수정",
            "cafe\u{301}",
        ] {
            assert_eq!(back("ok", text).description.as_deref(), Some(text));
            assert!(Label::plain(text, Label::DESCRIPTION_MOST), "{text}");
        }
        let long = back("ok", &"é".repeat(250)).description.unwrap();
        assert_eq!(
            long.chars().count(),
            Label::DESCRIPTION_MOST,
            "cut to its bound"
        );
        assert_eq!(
            back("ok", "\u{7}\u{202e}").description,
            None,
            "nothing left"
        );
    }

    /// **The derivation is pinned.** A stored id is never derived again (the
    /// default's is a value, `DEFAULT_EPISODE`), so the tag may change — it
    /// did on 11-08, to Shannon's spelling (Steven) — but only on purpose:
    /// this value moves with it, and nothing else may.
    #[test]
    fn the_episode_id_derivation_is_pinned() {
        assert_eq!(
            EpisodeId::from_seed(b"golden").to_hex(),
            "5f8b868889142d7a0552121ea5a4147d09247e4a57bccc242def0a344705d840"
        );
    }

    #[test]
    fn root_has_no_origin_and_empty_head() {
        let ep = Episode::root(EpisodeId::from_seed(b"root"), cfg());
        assert!(ep.origin.is_none());
        assert!(ep.own_is_empty());
        assert_eq!(ep.own_head(), None);
        assert!(ep.segments().is_empty());
    }

    #[test]
    fn head_is_the_last_appended_turn() {
        let mut ep = Episode::root(EpisodeId::from_seed(b"root"), cfg());
        let (a, b) = (occ("a"), occ("b"));
        ep.append(a.clone());
        ep.append(b.clone());
        assert_eq!(ep.own_len(), 2);
        assert_eq!(ep.own_head(), Some(b.turn));
        assert_eq!(ep.own_turns(), vec![a.turn, b.turn]);
        // Appends coalesce into one trailing Own segment.
        assert_eq!(ep.segments(), &[Segment::Own(vec![a, b])]);
    }

    #[test]
    fn branch_shares_a_prefix_slice_and_records_its_origin() {
        let parent_id = EpisodeId::from_seed(b"root");
        let at = turn_id("a");
        let origin = Origin {
            episode: parent_id,
            turn: at,
            index: Some(0),
            kind: EdgeKind::Branch,
        };
        let child = Episode::branch(EpisodeId::from_seed(b"child"), cfg(), origin, 1);
        assert_eq!(child.origin, Some(origin));
        assert_eq!(child.origin.unwrap().kind, EdgeKind::Branch);
        // A freshly forked branch owns nothing; it carries a single leading Slice
        // of the parent's first turn (the shared prefix, by reference).
        assert!(child.own_is_empty());
        assert_eq!(
            child.segments(),
            &[Segment::Slice {
                episode: parent_id,
                start: 0,
                end: 1,
            }]
        );
    }

    /// **Where a session came from reads as where it stands** — `at <turn>`
    /// for a branch with nothing of its own yet (a checkout looked at:
    /// S21-Q8), `branch ← <turn>` once it has a turn; a clone by the session
    /// it copies (F L8), a merge by where it forked, whatever each owns; a
    /// root nothing.
    #[test]
    fn a_checkout_reads_at_its_turn_until_it_owns_one() {
        let at = turn_id("a");
        let made = |kind| {
            let origin = Origin {
                episode: EpisodeId::from_seed(b"root"),
                turn: at,
                index: Some(0),
                kind,
            };
            Episode::branch(EpisodeId::from_seed(b"child"), cfg(), origin, 1)
        };
        let short = at.short_hex();
        let mut checkout = made(EdgeKind::Branch);
        assert_eq!(checkout.origin_clause(), Some(format!("at {short}")));
        checkout.append(occ("d"));
        assert_eq!(checkout.origin_clause(), Some(format!("branch ← {short}")));
        let source = EpisodeId::from_seed(b"root").short_hex();
        assert_eq!(
            made(EdgeKind::Clone).origin_clause(),
            Some(format!("clone of {source}"))
        );
        assert_eq!(
            made(EdgeKind::Merge).origin_clause(),
            Some(format!("merge ← {short}"))
        );
        let root = Episode::root(EpisodeId::from_seed(b"root"), cfg());
        assert_eq!(root.origin_clause(), None);
    }

    #[test]
    fn append_after_a_slice_opens_a_new_own_segment() {
        // A branch (leading Slice) that then diverges: the appended turn starts a
        // fresh Own piece rather than merging into the borrowed slice.
        let parent_id = EpisodeId::from_seed(b"root");
        let origin = Origin {
            episode: parent_id,
            turn: turn_id("a"),
            index: Some(1),
            kind: EdgeKind::Branch,
        };
        let mut child = Episode::branch(EpisodeId::from_seed(b"child"), cfg(), origin, 2);
        let d = occ("d");
        child.append(d.clone());
        assert_eq!(
            child.segments(),
            &[
                Segment::Slice {
                    episode: parent_id,
                    start: 0,
                    end: 2,
                },
                Segment::Own(vec![d.clone()]),
            ]
        );
        assert_eq!(child.own_turns(), vec![d.turn]);
        assert_eq!(child.own_head(), Some(d.turn));
    }

    #[test]
    fn from_seed_is_deterministic_and_seed_sensitive() {
        assert_eq!(EpisodeId::from_seed(b"x"), EpisodeId::from_seed(b"x"));
        assert_ne!(EpisodeId::from_seed(b"x"), EpisodeId::from_seed(b"y"));
    }

    #[test]
    fn id_to_hex_is_64_lowercase_hex_chars() {
        let hex = EpisodeId::from_seed(b"root").to_hex();
        assert_eq!(hex.len(), 64);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn episode_id_serializes_as_a_hex_string() {
        let id = EpisodeId::from_seed(b"x");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{}\"", id.to_hex()));
        assert_eq!(serde_json::from_str::<EpisodeId>(&json).unwrap(), id);
    }

    #[test]
    fn origin_roundtrips_through_json() {
        // The edge kind serializes as a readable snake_case tag.
        let origin = Origin {
            episode: EpisodeId::from_seed(b"p"),
            turn: turn_id("a"),
            index: Some(0),
            kind: EdgeKind::ToolCall,
        };
        let json = serde_json::to_string(&origin).unwrap();
        assert!(json.contains("tool_call"), "edge kind is readable: {json}");
        assert_eq!(serde_json::from_str::<Origin>(&json).unwrap(), origin);
    }

    #[test]
    fn segment_roundtrips_through_json() {
        // Both variants round-trip as derived. Not what the store writes —
        // the JSONL log is `Record::Turn` / `Record::Slice`, mapped by hand —
        // and nothing reads a `Segment` as JSON today; this said "see the
        // JSONL store" (review stop 12, T9).
        for seg in [
            Segment::Own(vec![occ("a"), occ("b")]),
            Segment::Slice {
                episode: EpisodeId::from_seed(b"s"),
                start: 1,
                end: 4,
            },
        ] {
            let json = serde_json::to_string(&seg).unwrap();
            assert_eq!(serde_json::from_str::<Segment>(&json).unwrap(), seg);
        }
    }
}
