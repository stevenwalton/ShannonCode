//! The atomic unit of the conversation graph: a content-addressed **Turn**.
//!
//! A Turn is one round-trip — the user's input plus the assistant's output (and,
//! once the tool surface lands in §7, the intermediate tool-call cycles within
//! the same Turn). Per design.md §1.1 it is **content-addressable**: its
//! [`TurnId`] is a hash of its messages plus the config that produced them, so
//! two turns with identical messages + config share an id (deduplication) and a
//! change to any hashed field changes the id. Identity is *local* to the turn —
//! conversation identity is the sequence of `TurnId`s in an `Episode` (like git:
//! a blob hashes its bytes, a commit chains them).
//!
//! **Hashed (identity) vs. provenance (metadata):** the messages and the
//! [`TurnConfig`] are hashed; the timestamp and the producing account are
//! recorded in [`Provenance`] but deliberately *not* hashed — two identical
//! turns produced at different times, or under different accounts, are still
//! "the same turn."
//!
//! **Same turn, two occurrences.** Identity being content is why provenance
//! cannot live beside the content in the graph: two productions of one turn
//! share its messages and NOT its facts — when each ran, what each cost, why
//! each stopped, what a result turn distilled. So a [`Turn`] is the sealed
//! RECORD of one production (content + its provenance — what a store writes
//! and reads back), and the graph splits it: the content joins a set keyed by
//! [`TurnId`], and the provenance travels as an [`Occurrence`] in the episode
//! that produced it. Keyed by id, the second production used to overwrite the
//! first's facts (`[[feedback_content_addressed_ids]]`).

use std::time::SystemTime;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::graph::{decode_hex32, encode_hex32};
use crate::message::{Block, ContextKind, Message, Role};
use crate::stream::{StopReason, Usage};

/// Content-address of a [`Turn`]: a 256-bit BLAKE3 hash of the turn's messages +
/// its [`TurnConfig`]. `Copy` (32 bytes) so it's cheap to pass around as a key.
///
/// Serde form is the **hex string** (not a 32-element byte array) so persisted
/// turns are `grep`-able by id — see [`to_hex`](Self::to_hex).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TurnId([u8; 32]);

impl TurnId {
    /// Compute a turn's id from the fields that define its identity. Every field
    /// is **length-prefixed** before hashing so the concatenation is
    /// unambiguous — hashing `["ab", "c"]` must differ from `["a", "bc"]`. The
    /// encoding is deterministic (fixed field order, little-endian lengths, no
    /// float formatting yet), which the unit tests pin.
    pub fn compute(messages: &[Message], config: &TurnConfig) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(&(messages.len() as u64).to_le_bytes());
        for m in messages {
            let role = match m.role {
                Role::User => 0u8,
                Role::Assistant => 1u8,
            };
            h.update(&[role]);
            update_field(&mut h, m.content.as_bytes());
            // Fold in any structured blocks. Empty for a plain text turn, so its
            // hash is byte-for-byte what it was before blocks existed — existing
            // content-addressed ids stay stable ([[content-addressed-ids]]).
            for block in &m.blocks {
                hash_block(&mut h, block);
            }
        }
        update_field(&mut h, config.model.as_bytes());
        // **`max_tokens` is provenance, not identity** — Steven's call, and the
        // same rule `Provenance::usage` already states: "two identical turns are
        // the same turn whatever they cost". A cap is a knob on the *request*;
        // if it truncated the reply, that truncation is already in `messages`,
        // which IS hashed. Two turns with identical messages and model are the
        // same turn whether or not a ceiling happened to be in effect.
        //
        // **The zero byte stays, and must.** Every turn ever written hashed a
        // `0` here (the field has been `None` at all four seal sites since it
        // existed), and `JsonlStore` re-verifies each turn's id against its
        // content on load and rejects the file as **corrupt** on a mismatch.
        // Dropping the byte would change every stored id and make every session
        // on disk unloadable. Same technique as the `blocks` loop above, whose
        // comment says it: keep the neutral element so existing
        // content-addresses stay stable. `a_stored_turns_id_never_changes` pins
        // it with a literal hash.
        h.update(&[0]);
        Self(*h.finalize().as_bytes())
    }

    /// The id as 64 lowercase hex chars — for display, refs, and `cat`-able logs.
    pub fn to_hex(&self) -> String {
        encode_hex32(&self.0)
    }

    /// The git-short form, through the shared rule in
    /// [`short_hex`](super::episode::short_hex) — see there for why it is a
    /// free function over a string rather than a slice repeated per type.
    pub fn short_hex(&self) -> String {
        super::episode::short_hex(&self.to_hex()).to_string()
    }

    /// Parse an id from 64 hex chars (either case); `None` if malformed. The
    /// inverse of [`to_hex`](Self::to_hex), used to read ids back from JSON.
    pub fn from_hex(s: &str) -> Option<Self> {
        decode_hex32(s).map(Self)
    }

    /// The raw 32 hash bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Serialize as the hex string so persisted turns are inspectable / `grep`-able.
impl Serialize for TurnId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for TurnId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let hex = String::deserialize(deserializer)?;
        Self::from_hex(&hex)
            .ok_or_else(|| serde::de::Error::custom("TurnId: expected 64 hex chars"))
    }
}

/// Length-prefixed byte field for the content-address hash, so variable-length
/// fields can't be confused across a boundary.
fn update_field(h: &mut blake3::Hasher, bytes: &[u8]) {
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

/// Fold one structured block into the content-address hash. A per-variant tag
/// byte plus length-prefixed fields keeps two blocks (or a block and adjacent
/// content) from ever hashing alike. `tool_use` input is hashed via its JSON
/// string; `serde_json`'s default `Value` map is key-sorted, so it is
/// deterministic for a given value.
fn hash_block(h: &mut blake3::Hasher, block: &Block) {
    match block {
        Block::Text { text } => {
            h.update(&[0u8]);
            update_field(h, text.as_bytes());
        }
        Block::ToolUse { id, name, input } => {
            h.update(&[1u8]);
            update_field(h, id.as_bytes());
            update_field(h, name.as_bytes());
            update_field(h, input.to_string().as_bytes());
        }
        Block::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            h.update(&[2u8]);
            update_field(h, tool_use_id.as_bytes());
            update_field(h, content.as_bytes());
            h.update(&[*is_error as u8]);
        }
        // A new tag, never a changed one: the store re-verifies every id on
        // load, so an existing tag's bytes are fixed forever.
        Block::Context { kind, source, text } => {
            h.update(&[3u8, context_byte(*kind)]);
            update_field(h, source.as_bytes());
            update_field(h, text.as_bytes());
        }
        // Thinking joins a turn's identity where it is present (navigation.md
        // §11, NQ1) — no turn stored before it had any, so no id moves — and
        // its place does too: the same blocks in another order are another
        // reply on the wire.
        Block::Thinking {
            thinking,
            signature,
            at,
        } => {
            h.update(&[4u8]);
            h.update(&(*at as u64).to_le_bytes());
            update_field(h, thinking.as_bytes());
            update_field(h, signature.as_bytes());
        }
        Block::RedactedThinking { data, at } => {
            h.update(&[5u8]);
            h.update(&(*at as u64).to_le_bytes());
            update_field(h, data.as_bytes());
        }
    }
}

/// **A context block's kind as the hash reads it** — a byte fixed forever,
/// never the word the model reads (`ContextKind::word`), which is prose and
/// may be reworded: rewording it changed every stored id, and the store drops
/// a session whose ids do not verify (Reviewer A, round 27). Exhaustive, so a
/// new kind takes a new byte by decision.
fn context_byte(kind: ContextKind) -> u8 {
    match kind {
        ContextKind::Instructions => 0,
        ContextKind::Memory => 1,
        ContextKind::Skills => 2,
        ContextKind::Skill => 3,
        ContextKind::Unloaded => 4,
    }
}

/// The model + sampling parameters that, with the messages, define a Turn's
/// identity. Starts with what `SendRequest` carries today (`model`,
/// `max_tokens`); the system prompt, tool definitions, and temperature join it
/// (and the hash) as they become real — earned-use, per design.md §1.1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnConfig {
    /// The resolved model that produced the turn (the concrete one used, not an
    /// override-or-default `Option`).
    pub model: String,
    /// Output-token cap in effect, if any.
    pub max_tokens: Option<u32>,
    /// On a turn, the effort it was asked for, if one was sent — `None`
    /// sent none (the model's own, or through Claude Code the user's own
    /// setting). On an EPISODE, the session's own effort — a preference, as
    /// `model` is there — `None` following the run's `[turn] effort`.
    /// Provenance, **not identity**, as `max_tokens` is: the reply it shaped
    /// is hashed already. Omitted when `None`, so a turn without one is
    /// written as it always was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
}

impl TurnConfig {
    /// **The seal of a turn no request produced** — written, imported or
    /// returned — on this config's model: no ceiling and no effort, because
    /// none was sent. Sealed under a session's preference instead, it claimed
    /// the session's effort as what produced it (Reviewers, 10-04). Its
    /// consumers: a Claude Code import, a hand-typed `s` summary, a `:return`
    /// result, the turns an edit adds.
    pub fn authored(&self) -> TurnConfig {
        TurnConfig {
            model: self.model.clone(),
            max_tokens: None,
            effort: None,
        }
    }
}

/// **How much work a model puts into a reply** — the API's
/// `output_config.effort` and `claude --effort`. Lowest first, so the order
/// is "more work". Which levels a model takes, and its default, are its row
/// in `backend::models` (verified against the API's effort page,
/// 2026-10-04).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    /// Every level, lowest first.
    pub const ALL: [Effort; 5] = [
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::Xhigh,
        Effort::Max,
    ];

    /// The level as the API and `claude` spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
            Effort::Max => "max",
        }
    }

    /// A level from how it is spelled, in any case; `None` for anything else.
    pub fn parse(word: &str) -> Option<Effort> {
        Effort::ALL
            .into_iter()
            .find(|e| e.as_str().eq_ignore_ascii_case(word.trim()))
    }

    /// **A setting's word** — a level, or `default` (or nothing) for none, as
    /// `[turn] effort` and `:effort` both spell it — and the one sentence for a
    /// word that is neither.
    pub fn parse_setting(word: &str) -> Result<Option<Effort>, String> {
        let word = word.trim();
        if word.is_empty() || word.eq_ignore_ascii_case("default") {
            return Ok(None);
        }
        Effort::parse(word).map(Some).ok_or_else(|| {
            // Built from the levels, so the sentence cannot list a stale set.
            let levels: Vec<&str> = Effort::ALL.iter().map(|e| e.as_str()).collect();
            format!(
                "`{word}` is not an effort — {}, or default",
                levels.join(", ")
            )
        })
    }
}

impl std::fmt::Display for Effort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a Turn came from. `config` is part of the turn's *identity* (hashed);
/// `at`, `account`, and `usage` are *metadata* (not hashed) — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// The config that produced the turn (also the hash input).
    pub config: TurnConfig,
    /// When the turn ran.
    pub at: SystemTime,
    /// Which account/credential produced it — the OAuth slot (design.md §1.4 /
    /// the OAuth milestone). `None` until subscription auth lands; the field
    /// exists now so the on-disk format needn't break when it does.
    pub account: Option<String>,
    /// Token usage the backend reported for the turn (input ↑ / output ↓).
    /// Metadata, **not** hashed — two identical turns are the same turn whatever
    /// they cost. `None` for a turn whose backend reported nothing (or a turn
    /// persisted before usage was recorded); `#[serde(default)]` so old logs
    /// deserialize. Filled by `Conversation::commit_turn` from the stream.
    #[serde(default)]
    pub usage: Option<Usage>,
    /// The turn's **final request alone** — what the next request on this
    /// session re-sends, and therefore what "how full is the window" is asking
    /// about.
    ///
    /// **A second interval over the same numbers, persisted because it cannot
    /// be recovered from the first.** [`usage`](Self::usage) is the whole turn,
    /// summed over its tool rounds; each round's prompt already contains every
    /// earlier one, so adding them counts the shared prefix N times. The live
    /// path has taken both since `Billing::record` was split in two — the
    /// *replay* path could not, because only the sum ever reached disk, so a
    /// resumed two-round turn read 2710 where the honest figure is 1210. The
    /// comment there claimed the two intervals coincide on replay; they
    /// coincide only for a single-round turn, which is most of them and so hid
    /// it (`[[feedback_fix_neighbourhood]]`).
    ///
    /// `None` for turns written before this field existed — those fall back to
    /// [`usage`](Self::usage), which is exact for a single-round turn and
    /// over-reports a multi-round one exactly as it did before. Metadata,
    /// **not** hashed; `#[serde(default)]` so old logs deserialize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_round: Option<Usage>,
    /// **What the turn was answered after** — the history its
    /// [`last_round`](Self::last_round) measured. A window is one history's:
    /// replayed onto another (a `:compact` keeps a turn and replaces what
    /// came before it), it describes a window that no longer exists, and
    /// `ctx` read the old 100% over an emptied session (Steven, 10-15). So a
    /// measurement is trusted where its turn still follows this, and the
    /// window is estimated elsewhere (`crate::window`).
    ///
    /// Set where a window is recorded: a live commit and a continuation.
    /// `None` for every other seal (a summary, an import — none measures a
    /// window) and for a turn written before this field, which is trusted as
    /// it always was. Metadata, **not** hashed; `serde(default)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follows: Option<Follows>,
    /// Why the turn ended (`end_turn`, `max_tokens`,
    /// [`StopReason::Interrupted`] for a sealed partial, …). Metadata, **not**
    /// hashed — the same text is the same content whatever cut it off — and
    /// `serde(default)` so old logs deserialize as `None`. This is what makes
    /// a truncated reply visibly truncated, and gives `:continue` ground truth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    /// For a **squash summary**: the content-addresses of the turns it distilled —
    /// the machine-readable record of what this summary stands in for, surfaced by
    /// the `:graph` inspector. Empty for ordinary turns. Metadata, **not** hashed
    /// (same summary text = same turn, whatever it summarized), and it survives the
    /// user rewriting the summary text in `$EDITOR`. Ids here are *references to
    /// content* — the one correct use of a content-address (`[[content-addressed-ids]]`);
    /// the sources stay resolvable because squash preserves the original episodes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub distilled_from: Vec<TurnId>,
    /// **The secret handles this turn names that the session held when it was
    /// sealed** — what the app itself put there (a value masked into the
    /// prompt) or the model spent from the list it was given. The ONLY handles
    /// a resumed history may ask the vault about.
    ///
    /// Provenance, not shape: recall used to ask about every handle-shaped
    /// string in the history, so a tool's output carrying guesses walked the
    /// 16-bit nonce and pulled another project's kept secret into the session
    /// (Reviewer E, round 18). A guess was never held, so it is never recorded.
    ///
    /// **Fail-closed by construction:** [`Turn::seal`] starts it at
    /// `Some(vec![])` — held nothing — so a seal site that forgets to say what
    /// it held leaves its handles unrestorable (visible: they stop working on
    /// resume) rather than reopening a scan. `None` therefore means one thing
    /// only: a turn read from a log written before this field existed. Recall
    /// falls back for those to the turn's own text (never a tool's output),
    /// capped. Metadata, **not** hashed; `serde(default)` so old logs
    /// deserialize as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handles: Option<Vec<String>>,
}

/// Where in a history a turn was answered: first, or after a given turn —
/// [`Provenance::follows`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Follows {
    /// The history's first turn.
    Start,
    /// After this turn.
    Turn(TurnId),
}

/// One **production** of a turn: which content, and the facts of producing it
/// there. It lives in the episode that produced it (a [`Segment::Own`] entry)
/// and nowhere else; a `Slice` that shares the turn shares this occurrence, so
/// borrowed history keeps the facts of where it was made.
///
/// [`Segment::Own`]: super::Segment::Own
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Occurrence {
    /// The content, by its address.
    pub turn: TurnId,
    /// What produced it, when, at what cost, and how it ended.
    pub provenance: Provenance,
}

/// One round-trip in a conversation, content-addressed by [`id`](Self::id) —
/// the sealed RECORD of one production: its content and that production's
/// [`Provenance`]. What a store writes per turn and reads back; the graph
/// holds the two halves apart (see the module docs).
///
/// Not `Eq` (a message's `tool_use` input is arbitrary JSON); identity is the
/// content-address [`id`](Self::id), not structural equality.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    /// Content-address of this turn (its messages + config).
    pub id: TurnId,
    /// The turn's messages: the user input then the assistant output (tool-call
    /// cycles will join here when §7 lands).
    pub messages: Vec<Message>,
    /// The model/account/time the turn was produced under.
    pub provenance: Provenance,
}

impl Turn {
    /// Seal `messages` produced under `config` at time `at` into a Turn,
    /// computing its content-address. `account` starts `None` (the auth layer
    /// fills it when OAuth lands); `handles` starts as **none held** — see
    /// [`Provenance::handles`] and [`with_handles`](Self::with_handles).
    pub fn seal(messages: Vec<Message>, config: TurnConfig, at: SystemTime) -> Self {
        // As the store will read it back, so the id it gets is the id the
        // load computes (`Message::into_read_back`; review stop 15, E H1).
        let messages: Vec<Message> = messages.into_iter().map(Message::into_read_back).collect();
        let id = TurnId::compute(&messages, &config);
        Self {
            id,
            messages,
            provenance: Provenance {
                config,
                at,
                account: None,
                usage: None,
                last_round: None,
                follows: None,
                stop_reason: None,
                distilled_from: Vec::new(),
                handles: Some(Vec::new()),
            },
        }
    }

    /// This record's occurrence — the half an episode holds.
    pub fn occurrence(&self) -> Occurrence {
        Occurrence {
            turn: self.id,
            provenance: self.provenance.clone(),
        }
    }

    /// The record of `occurrence`, given its content — the inverse of the
    /// graph's split, for whatever writes or replays a whole turn.
    pub fn of(occurrence: &Occurrence, messages: Vec<Message>) -> Turn {
        Turn {
            id: occurrence.turn,
            messages,
            provenance: occurrence.provenance.clone(),
        }
    }

    /// This turn marked as a **distillation** of `sources` (a squash summary's
    /// provenance). A builder over metadata, so the content-address is untouched —
    /// call it after [`seal`](Self::seal), before the turn enters a composition.
    pub fn with_distilled_from(mut self, sources: Vec<TurnId>) -> Turn {
        self.provenance.distilled_from = sources;
        self
    }

    /// This turn, recording the held secret handles it names — see
    /// [`Provenance::handles`].
    pub fn with_handles(mut self, handles: Vec<String>) -> Turn {
        self.provenance.handles = Some(handles);
        self
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

    /// **One spelling per level**: what serde writes (the wire, the file) is
    /// `as_str`, which `parse` and `parse_setting` read back — two lists that
    /// could drift apart if a variant were renamed in one.
    #[test]
    fn an_effort_is_spelled_one_way() {
        for e in Effort::ALL {
            assert_eq!(serde_json::to_value(e).unwrap(), e.as_str());
            assert_eq!(Effort::parse(e.as_str()), Some(e));
            assert_eq!(
                Effort::parse_setting(&e.as_str().to_uppercase()),
                Ok(Some(e))
            );
        }
        assert_eq!(Effort::parse_setting(" Default "), Ok(None));
        assert_eq!(Effort::parse_setting(""), Ok(None));
        let why = Effort::parse_setting("hgih").unwrap_err();
        assert_eq!(
            why,
            "`hgih` is not an effort — low, medium, high, xhigh, max, or default"
        );
    }

    fn msgs() -> Vec<Message> {
        vec![Message::user("hi"), Message::assistant("hello")]
    }

    #[test]
    fn same_messages_and_config_hash_equal() {
        // Content-addressing is deterministic: identical inputs → identical id.
        assert_eq!(
            TurnId::compute(&msgs(), &cfg()),
            TurnId::compute(&msgs(), &cfg())
        );
    }

    #[test]
    fn changing_any_hashed_field_changes_the_id() {
        let base = TurnId::compute(&msgs(), &cfg());

        // The assistant output differs.
        let other_output = vec![Message::user("hi"), Message::assistant("HELLO")];
        assert_ne!(TurnId::compute(&other_output, &cfg()), base, "output");

        // The model differs.
        let other_model = TurnConfig {
            model: "claude-opus".into(),
            ..cfg()
        };
        assert_ne!(TurnId::compute(&msgs(), &other_model), base, "model");

        // **The token cap does NOT.** It is provenance, like `usage`: a knob on
        // the request, whose effect on the reply is already in `messages`. This
        // asserted the opposite until 2026-08-25 — the change is deliberate.
        let other_tokens = TurnConfig {
            max_tokens: Some(1024),
            ..cfg()
        };
        assert_eq!(
            TurnId::compute(&msgs(), &other_tokens),
            base,
            "a cap is provenance, not identity"
        );

        // What the model was told is part of what produced the reply: a
        // context block, and each of its fields.
        let told = |kind, source: &str, text: &str| {
            let mut m = msgs();
            m[0].blocks.push(Block::Context {
                kind,
                source: source.into(),
                text: text.into(),
            });
            TurnId::compute(&m, &cfg())
        };
        let one = told(crate::ContextKind::Memory, "/a", "x");
        assert_ne!(one, base, "a context block");
        assert_ne!(
            told(crate::ContextKind::Instructions, "/a", "x"),
            one,
            "kind"
        );
        assert_ne!(told(crate::ContextKind::Memory, "/b", "x"), one, "source");
        assert_ne!(told(crate::ContextKind::Memory, "/a", "y"), one, "text");
    }

    /// **A turn already on disk must keep its id, forever.**
    ///
    /// `JsonlStore::load` recomputes each turn's hash from its stored content
    /// and rejects the file as *corrupt* on a mismatch, so any change to
    /// `compute` that alters an existing id does not degrade — it makes every
    /// session unloadable. That is why `compute` still writes a `0` byte for a
    /// field it no longer reads.
    ///
    /// A literal hash rather than a property: a self-consistent test cannot
    /// catch this, because both sides move together.
    #[test]
    fn a_stored_turns_id_never_changes() {
        assert_eq!(
            TurnId::compute(&msgs(), &cfg()).to_hex(),
            "3b7232a95d0606fbd991bd8b274fbdee5ff019055fcbbf4bdcce0c737d327bda",
            "an id written by an older build must still verify — see `compute`"
        );
    }

    /// **A turn told its context keeps its id** — every kind, pinned, so a
    /// reworded `ContextKind::word` (or any change to `context_byte`) fails
    /// here rather than in a user's store.
    #[test]
    fn a_context_turns_id_never_changes() {
        let kinds = [
            ContextKind::Instructions,
            ContextKind::Memory,
            ContextKind::Skills,
            ContextKind::Skill,
            ContextKind::Unloaded,
        ];
        let mut user = Message::user("go");
        user.blocks = kinds
            .iter()
            .map(|&kind| Block::Context {
                kind,
                source: "/s".into(),
                text: "t".into(),
            })
            .collect();
        let messages = vec![user, Message::assistant("ok")];
        assert_eq!(
            TurnId::compute(&messages, &cfg()).to_hex(),
            "95f03bb995451f4f18de69f6ec5117cc89f52a4a8b4ac280ae7bfe2a09f2458b",
            "a stored id must still verify"
        );
    }

    /// **A turn holding every kind of block keeps its id** — a tool call and
    /// both answers to one beside text and context, as an agent's history is
    /// mostly made of. Only text and context turns were pinned: a changed
    /// `ToolUse` or `ToolResult` tag passed all 3546 tests, and would have
    /// made every stored session with a tool call fail its load (review stop
    /// 12, T9 3). The `match` below fails to compile on a new kind until it
    /// is pinned here.
    #[test]
    fn a_turn_with_every_kind_of_block_keeps_its_id() {
        let blocks = vec![
            Block::Text { text: "t".into() },
            Block::ToolUse {
                id: "tu".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "ls", "a": [1, null]}),
            },
            Block::ToolResult {
                tool_use_id: "tu".into(),
                content: "out".into(),
                is_error: false,
            },
            Block::ToolResult {
                tool_use_id: "tu".into(),
                content: "refused".into(),
                is_error: true,
            },
            Block::Context {
                kind: ContextKind::Memory,
                source: "/m".into(),
                text: "remember".into(),
            },
        ];
        for block in &blocks {
            match block {
                Block::Text { .. }
                | Block::ToolUse { .. }
                | Block::ToolResult { .. }
                | Block::Context { .. } => {}
                // New tags (4, 5), so no stored id holds one: pinned by
                // `thinking_and_its_place_are_identity`, not here.
                Block::Thinking { .. } | Block::RedactedThinking { .. } => {}
            }
        }
        let mut call = Message::assistant("calling");
        call.blocks = blocks;
        let messages = vec![Message::user("go"), call];
        assert_eq!(
            TurnId::compute(&messages, &cfg()).to_hex(),
            "5061d979d5c4deef97b52cc435b99afa1ecfa2824a7a95d676aee457d8222916",
            "a stored id must still verify"
        );
    }

    /// **Thinking is part of a turn, and so is where it stands** (NQ1): the
    /// same reply with it, without it, or with it after the text instead of
    /// before, are three turns — on the wire they are three replies.
    #[test]
    fn thinking_and_its_place_are_identity() {
        let reply = |blocks: Vec<Block>| {
            let mut m = Message::assistant("done");
            m.blocks = blocks;
            TurnId::compute(&[Message::user("go"), m], &cfg())
        };
        let thought = |at| Block::Thinking {
            thinking: "plan".into(),
            signature: "S".into(),
            at,
        };
        let plain = reply(vec![]);
        let before = reply(vec![thought(0)]);
        let after = reply(vec![thought(4)]);
        let redacted = reply(vec![Block::RedactedThinking {
            data: "plan".into(),
            at: 0,
        }]);
        assert_ne!(plain, before);
        assert_ne!(before, after, "its place");
        assert_ne!(before, redacted, "its kind");
    }

    /// **A sealed turn re-verifies as the store reads it back, whatever set
    /// its thoughts' places** — past the text's end, inside a character, out
    /// of order: the file keeps no place, only the order (`MessageRepr`), and
    /// the load hashes what it re-derives. Sealed over the places as given,
    /// a turn whose place a rewrite of its text had left behind failed its
    /// content hash on load, and its whole episode was skipped (review stop
    /// 15, E H1: the branch an `e` made, gone at the next launch).
    #[test]
    fn a_sealed_turn_re_verifies_as_it_is_read_back() {
        let thought = |at| Block::Thinking {
            thinking: "plan".into(),
            signature: "S".into(),
            at,
        };
        for places in [vec![40], vec![2], vec![3, 1], vec![1, 40]] {
            let mut m = Message::assistant("dön¡e");
            m.blocks = places.iter().map(|&at| thought(at)).collect();
            let turn = Turn::seal(vec![Message::user("go"), m], cfg(), SystemTime::UNIX_EPOCH);
            let json = serde_json::to_string(&turn.messages).unwrap();
            let read: Vec<Message> = serde_json::from_str(&json).unwrap();
            assert_eq!(read, turn.messages, "{places:?}: kept as read");
            assert_eq!(TurnId::compute(&read, &cfg()), turn.id, "{places:?}");
        }
    }

    #[test]
    fn length_prefixing_disambiguates_concatenation() {
        // Two user messages "ab","c" must not collide with "a","bc".
        let x = vec![Message::user("ab"), Message::user("c")];
        let y = vec![Message::user("a"), Message::user("bc")];
        assert_ne!(TurnId::compute(&x, &cfg()), TurnId::compute(&y, &cfg()));
    }

    #[test]
    fn provenance_metadata_is_not_part_of_identity() {
        // Same messages + config, different time and account → same id.
        let t1 = Turn::seal(msgs(), cfg(), SystemTime::UNIX_EPOCH);
        let mut t2 = Turn::seal(msgs(), cfg(), SystemTime::now());
        t2.provenance.account = Some("acct-123".into());
        assert_eq!(t1.id, t2.id, "timestamp + account don't affect identity");
        assert_eq!(t1.id, TurnId::compute(&msgs(), &cfg()), "seal uses compute");
    }

    #[test]
    fn distilled_from_is_metadata_and_round_trips() {
        let sources = vec![TurnId::compute(&msgs(), &cfg())];
        let plain = Turn::seal(msgs(), cfg(), SystemTime::UNIX_EPOCH);
        let summary = plain.clone().with_distilled_from(sources.clone());
        assert_eq!(
            summary.id, plain.id,
            "distillation provenance doesn't change identity"
        );
        assert_eq!(summary.provenance.distilled_from, sources);

        // Survives persistence.
        let json = serde_json::to_string(&summary).unwrap();
        let back: Turn = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provenance.distilled_from, sources);

        // Back-compat: a record persisted before the field existed (no
        // `distilled_from` key) deserializes with the empty default — and an
        // ordinary turn omits the key entirely (old binaries keep reading us).
        let old = json.replace(
            &format!(
                ",\"distilled_from\":{}",
                serde_json::to_string(&sources).unwrap()
            ),
            "",
        );
        let from_old: Turn = serde_json::from_str(&old).expect("old-log record parses");
        assert!(from_old.provenance.distilled_from.is_empty());
        assert!(
            !serde_json::to_string(&plain)
                .unwrap()
                .contains("distilled_from")
        );
    }

    #[test]
    fn to_hex_is_64_lowercase_hex_chars() {
        let hex = TurnId::compute(&msgs(), &cfg()).to_hex();
        assert_eq!(hex.len(), 64);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn turn_id_serializes_as_a_hex_string() {
        // Persisted ids must be greppable text, not a 32-element byte array.
        let id = TurnId::compute(&msgs(), &cfg());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{}\"", id.to_hex()));
        assert_eq!(serde_json::from_str::<TurnId>(&json).unwrap(), id);
    }

    #[test]
    fn from_hex_is_the_inverse_of_to_hex_and_rejects_junk() {
        let id = TurnId::compute(&msgs(), &cfg());
        assert_eq!(TurnId::from_hex(&id.to_hex()), Some(id));
        assert_eq!(TurnId::from_hex("nothex"), None);
        assert_eq!(TurnId::from_hex(&"z".repeat(64)), None);
    }

    #[test]
    fn turn_roundtrips_through_json_with_provenance() {
        // The whole sealed turn — messages, config, and provenance — survives a
        // JSON round-trip, and the id appears verbatim for `grep`.
        let t = Turn::seal(msgs(), cfg(), SystemTime::UNIX_EPOCH);
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains(&t.id.to_hex()), "id is greppable in the line");
        assert_eq!(serde_json::from_str::<Turn>(&json).unwrap(), t);
    }

    #[test]
    fn provenance_usage_roundtrips_and_old_logs_default_to_none() {
        // A populated usage survives a round-trip...
        let mut t = Turn::seal(msgs(), cfg(), SystemTime::UNIX_EPOCH);
        t.provenance.usage = Some(crate::Usage {
            input_tokens: 7,
            output_tokens: 3,
            ..Default::default()
        });
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<Turn>(&json).unwrap(), t);
        // ...and a log written before `usage` existed (the key absent) parses,
        // defaulting to None — the `#[serde(default)]` forward-compat guarantee.
        let none = Turn::seal(msgs(), cfg(), SystemTime::UNIX_EPOCH);
        let dropped = serde_json::to_string(&none)
            .unwrap()
            .replace(",\"usage\":null", "");
        assert!(!dropped.contains("usage"), "the usage key is gone");
        let parsed: Turn = serde_json::from_str(&dropped).unwrap();
        assert_eq!(parsed, none, "missing usage defaults to None");
    }

    /// **A seal holds nothing until told — and says so on disk.** The default
    /// is `Some([])`, not `None`, so a seal site that forgets `with_handles`
    /// fails closed (nothing to restore) instead of landing on recall's legacy
    /// scan; it serializes as `[]`, which a log older than the field never
    /// wrote, so only that log reads back as `None`.
    #[test]
    fn a_seal_records_none_held_and_only_an_old_log_reads_as_legacy() {
        let t = Turn::seal(msgs(), cfg(), SystemTime::UNIX_EPOCH);
        assert_eq!(t.provenance.handles, Some(vec![]), "fail-closed default");
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"handles\":[]"), "{json}");
        assert_eq!(serde_json::from_str::<Turn>(&json).unwrap(), t);

        let held = t.clone().with_handles(vec!["{{pw.beef}}".into()]);
        let json = serde_json::to_string(&held).unwrap();
        assert_eq!(serde_json::from_str::<Turn>(&json).unwrap(), held);

        let old = serde_json::to_string(&t)
            .unwrap()
            .replace(",\"handles\":[]", "");
        assert!(!old.contains("handles"), "premise: the key is gone");
        let parsed: Turn = serde_json::from_str(&old).unwrap();
        assert_eq!(parsed.provenance.handles, None, "legacy");
    }

    // The canonical serialization is the subtle part of content-addressing
    // (design.md §1.1) — these property tests pin the two things our *encoding*
    // (not BLAKE3) is responsible for: it is deterministic, and it is injective
    // (distinct structured inputs never collapse to the same byte stream, which
    // would show up as an id collision). Arbitrary content — including the bytes
    // we use as length prefixes and role tags — must not confuse field
    // boundaries.
    mod props {
        use super::*;
        use proptest::prelude::*;

        fn arb_message() -> impl Strategy<Value = Message> {
            (any::<bool>(), any::<String>()).prop_map(|(is_user, content)| {
                if is_user {
                    Message::user(content)
                } else {
                    Message::assistant(content)
                }
            })
        }

        fn arb_effort() -> impl Strategy<Value = Option<Effort>> {
            proptest::option::of(proptest::sample::select(Effort::ALL.to_vec()))
        }

        fn arb_config() -> impl Strategy<Value = TurnConfig> {
            (any::<String>(), any::<Option<u32>>(), arb_effort()).prop_map(
                |(model, max_tokens, effort)| TurnConfig {
                    model,
                    max_tokens,
                    effort,
                },
            )
        }

        fn arb_input() -> impl Strategy<Value = (Vec<Message>, TurnConfig)> {
            (proptest::collection::vec(arb_message(), 0..6), arb_config())
        }

        proptest! {
            #[test]
            fn compute_is_deterministic((msgs, config) in arb_input()) {
                prop_assert_eq!(
                    TurnId::compute(&msgs, &config),
                    TurnId::compute(&msgs, &config),
                );
            }

            #[test]
            fn distinct_inputs_get_distinct_ids(a in arb_input(), b in arb_input()) {
                // BLAKE3 collisions are negligible (~2^-256), so an equal id on
                // inputs differing in an IDENTITY-bearing field means our
                // encoding lost a boundary.
                //
                // **It is the weaker of the two guards on that, though.**
                // Deleting the length prefix in `update_field` — the exact
                // `["ab","c"]` vs `["a","bc"]` confusion this is named for —
                // survives here, because proptest has to draw that pair with
                // everything else equal to notice. `a_stored_turns_id_never_changes`
                // is what actually catches it (mutation-verified, both ways).
                //
                // **`max_tokens` is not one of them**, by the decision recorded
                // in `TurnId::compute`: a cap is a knob on the request, and if
                // it truncated the reply that truncation is already in
                // `messages`. Comparing whole tuples asserted the opposite, so
                // this failed roughly one run in a hundred — whenever proptest
                // drew two inputs alike in model and messages and differing
                // only in the cap, which shrinks to `None` vs `Some(0)`. The
                // property below pins the other half, so excluding the field
                // here loses no coverage. Nor is `effort`, for the same reason
                // (`effort_never_changes_the_id`).
                let identity = |(msgs, cfg): &(Vec<Message>, TurnConfig)| {
                    (msgs.clone(), cfg.model.clone())
                };
                if identity(&a) != identity(&b) {
                    prop_assert_ne!(
                        TurnId::compute(&a.0, &a.1),
                        TurnId::compute(&b.0, &b.1),
                    );
                }
            }

            /// The deliberate half: a cap does not change what a turn IS.
            #[test]
            fn max_tokens_never_changes_the_id(
                (msgs, config) in arb_input(),
                other in any::<Option<u32>>(),
            ) {
                // Stated in `TurnId::compute` and, until this test, pinned
                // nowhere — which is why the natural "fix" for the property
                // above is to start hashing `max_tokens`. That would change
                // every id ever written, and `JsonlStore` re-verifies each
                // turn's id on load and rejects a mismatch as corrupt, so it
                // would make every stored session unloadable.
                let mut varied = config.clone();
                varied.max_tokens = other;
                prop_assert_eq!(
                    TurnId::compute(&msgs, &config),
                    TurnId::compute(&msgs, &varied),
                );
            }

            /// **`effort` is provenance, not identity**, as `max_tokens` is
            /// (`TurnConfig::effort`): hashing it when set would change every
            /// stored id of a turn sent with one, and the store rejects a
            /// mismatch as corrupt. Held by nothing until review stop 12 (T9
            /// 3: hashing it passed every test).
            #[test]
            fn effort_never_changes_the_id(
                (msgs, config) in arb_input(),
                other in arb_effort(),
            ) {
                let mut varied = config.clone();
                varied.effort = other;
                prop_assert_eq!(
                    TurnId::compute(&msgs, &config),
                    TurnId::compute(&msgs, &varied),
                );
            }
        }
    }
}
