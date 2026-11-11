//! Persistence for the conversation graph — the swappable [`Store`] trait.
//!
//! design.md §1.3.1 makes storage a *physical* concern hidden behind a small
//! trait so the backend can be swapped (today JSONL, tomorrow SQLite, a research
//! fork's Postgres) without touching graph semantics: "the CLI, the TUI, the
//! orchestrator, and plugins all consume `&dyn Store` rather than knowing about
//! JSONL." Two backends ship in M3 — [`MemoryStore`] (tests) and
//! [`JsonlStore`] (prod) — and they are held to **one shared
//! contract** (the `#[cfg(test)] contract` module), which is the proof that the
//! model, not the format, is what callers depend on.
//!
//! **The content-addressed split (why `put_episode` only persists metadata).**
//! A [`Turn`] carries its content and is addressed by its hash; an [`Episode`]
//! references turns *by id* and is a mutable, append-only sequence. So an
//! episode's bytes are: a little metadata (id, config, parent edge) plus an
//! ordered list of turn ids. [`Store::put_episode`] persists only that
//! metadata; the turn sequence is built up by [`Store::append_turn`], one turn
//! at a time. That is what lets the JSONL backend be a genuine append-only log
//! (`cat`/`tail -f`-able) instead of rewriting the whole episode per commit.
//!
//! **No `graph.json` at rest (M3).** The in-memory [`Graph`] is *rebuilt at
//! startup* by [`Store::load_graph`], which scans every episode. An at-rest DAG
//! cache is a measured-later optimization, deliberately out of M3 — the perf
//! rule is that this happens at load, never per frame.

use std::path::{Path, PathBuf};

use tracing::warn;

use crate::graph::{Episode, EpisodeId, Graph, Turn, TurnId};
use crate::message::Message;

/// A `Store` that can be told to fail — behind `test-util`.
#[cfg(any(test, feature = "test-util"))]
pub mod failing;
pub mod jsonl;
/// Any `Store`, made to load structure without bodies — behind `test-util`.
#[cfg(any(test, feature = "test-util"))]
pub mod lazy;
pub mod memory;

#[cfg(any(test, feature = "test-util"))]
pub use failing::FailingStore;
pub use jsonl::JsonlStore;
#[cfg(any(test, feature = "test-util"))]
pub use lazy::LazyBodies;
pub use memory::MemoryStore;

/// What a backend returns for a loaded episode: the reconstructed [`Episode`]
/// (metadata + its own turn-id order) together with the actual [`Turn`] contents
/// in that order. The caller can fold this straight into a [`Graph`]. Note these
/// are the episode's **own** (divergent) turns — a branch's shared prefix is not
/// stored per episode; derive the full history with
/// [`Graph::full_turns`](crate::Graph::full_turns) after the graph is assembled.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedEpisode {
    /// The episode, rebuilt from its metadata and the appended turn order.
    pub episode: Episode,
    /// The episode's own turns, oldest first — same order as
    /// [`episode.own_turns()`](Episode::own_turns).
    pub turns: Vec<Turn>,
}

/// A categorized persistence failure. Distinct from [`BackendError`] (which must
/// be `Clone + Eq` to ride inside `StreamMsg`); a store error wraps
/// `std::io::Error` / `serde_json::Error`, which are neither.
///
/// [`BackendError`]: crate::error::BackendError
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A read/append referenced an episode the store has never seen. The payload
    /// is the id's hex for a `grep`-able message.
    #[error("episode not found: {0}")]
    EpisodeNotFound(String),

    /// A ref name unsafe as a flat filename (see `validate_ref_name`).
    #[error("invalid ref name: {0:?}")]
    InvalidRef(String),

    /// On-disk data that parsed but is internally inconsistent — e.g. an episode
    /// referencing a turn that isn't stored. Distinct from a parse error so a
    /// caller can tell "the bytes are corrupt" from "the bytes don't decode."
    #[error("corrupt episode {episode}: {detail}")]
    Corrupt { episode: String, detail: String },

    /// Underlying filesystem failure (open / read / write / create dir).
    #[error("store i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// A record on disk could not be (de)serialized as JSON.
    #[error("store serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// A record nested deeper than the store reads back, refused at the
    /// write: written, it would have failed its session's load later, and
    /// every turn after it (review stop 4, E 3).
    #[error("a record nested {depth} deep, past the {limit} the store can read back")]
    TooDeep { depth: usize, limit: usize },
}

/// Result alias for store operations.
pub type StoreResult<T> = Result<T, StoreError>;

/// All conversation-graph persistence goes through this trait, so a backend is a
/// single `impl Store` swap (design.md §1.3.1). The surface is deliberately
/// small: episodes and turns by id, named refs, and a whole-graph load. The
/// edit *overlay* (`{supersedes, with}`) and compaction hooks sketched in the
/// design are earned later (the ops phase, P5); they are not part of the M3
/// contract.
/// Guard that every own-turn id in `episode` has its content in `own`.
///
/// A [`rewrite_episode`](Store::rewrite_episode) is the one write that can
/// *shorten* a log, so a missing turn here does not fail loudly later — it
/// produces a shorter session on the next load, which reads as data the user
/// lost rather than a write that was refused. Shared by every implementation so
/// a store double cannot accept a log the real store would reject.
pub(crate) fn check_own_present(episode: &Episode, own: &[Turn]) -> StoreResult<()> {
    for occurrence in episode.own_occurrences() {
        if !own.iter().any(|t| t.id == occurrence.turn) {
            return Err(StoreError::Corrupt {
                episode: episode.id.to_hex(),
                detail: format!(
                    "rewrite is missing the content of own turn {}",
                    occurrence.turn.to_hex()
                ),
            });
        }
    }
    Ok(())
}

/// The deepest a record nests that the store reads back: serde_json's reader
/// refuses a 128th level (and writes any depth).
const READ_DEPTH: usize = 127;

/// **`json`, a record as written, if a store can read it back** — or why not.
/// Shared by every implementation, as [`check_own_present`] is, so a store
/// double cannot keep a turn the real store refuses: JSONL measures each line
/// it writes, [`MemoryStore`] each turn it keeps (a `turn` line is the turn's
/// own object, tagged, so it nests as the turn does). Only a turn can nest
/// without bound — a call's input is any JSON; a header and a slice are fixed
/// shapes. `MemoryStore` had no limit, so no test on it reached
/// [`StoreError::TooDeep`] (review stop 12, T9 7).
pub(crate) fn check_readable(json: &str) -> StoreResult<()> {
    let depth = nesting(json);
    if depth > READ_DEPTH {
        return Err(StoreError::TooDeep {
            depth,
            limit: READ_DEPTH,
        });
    }
    Ok(())
}

/// How deep `json` nests — its `[` and `{` levels, strings read past.
fn nesting(json: &str) -> usize {
    let (mut depth, mut deepest) = (0usize, 0usize);
    let (mut in_string, mut escaped) = (false, false);
    for b in json.bytes() {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

pub trait Store {
    /// Persist an episode's **metadata** (id, config, origin edge). Idempotent:
    /// re-putting an existing episode updates its metadata and leaves its
    /// already-appended segments intact. The segment sequence is *not* taken from
    /// the passed episode — it is established only by the append methods, so the log
    /// stays append-only (`cat`/`tail -f`-able).
    fn put_episode(&mut self, episode: &Episode) -> StoreResult<()>;

    /// Append a sealed turn to an episode's log — extending its trailing
    /// [`Segment::Own`](crate::graph::Segment::Own), or opening a new one after a
    /// slice. Stores the turn's *content*. Errors with
    /// [`StoreError::EpisodeNotFound`] if the episode was never `put_episode`'d.
    fn append_turn(&mut self, episode: &EpisodeId, turn: &Turn) -> StoreResult<()>;

    /// Append a **shared slice** `[start, end)` of `source`'s derived sequence to an
    /// episode's log (a [`Segment::Slice`](crate::graph::Segment::Slice)) — the
    /// structural-sharing record. No turn content is written: the slice references
    /// `source`'s turns by id, so a branch's prefix or a re-root's suffix costs one
    /// small record, never a copy. Errors with [`StoreError::EpisodeNotFound`] if
    /// the episode was never `put_episode`'d.
    fn append_slice(
        &mut self,
        episode: &EpisodeId,
        source: &EpisodeId,
        start: usize,
        end: usize,
    ) -> StoreResult<()>;

    /// **Replace** an episode's log with `episode`'s current segments, writing
    /// `own` as the content of its [`Segment::Own`](crate::graph::Segment::Own)
    /// runs. The one operation that is not append-only.
    ///
    /// Removing a turn from the middle of a session cannot be expressed by
    /// appending: the log *is* the segment sequence, replayed in order, so there
    /// is no record that says "and now that one is gone". Everything else that
    /// edits history — branch, squash, re-root — mints a NEW episode and leaves
    /// the old log untouched, which is why this is the only rewrite in the
    /// trait.
    ///
    /// **Must be atomic.** A delete-then-replay would leave a window where the
    /// session exists nowhere on disk, and a crash inside it destroys history
    /// the user asked to *edit*, not to lose. Implementations write a complete
    /// replacement and swap it in; a failure leaves the original log intact.
    ///
    /// `own` must contain a [`Turn`] for every id in the episode's `Own`
    /// segments; ids with no turn supplied are an
    /// [`StoreError::Corrupt`], because writing a
    /// log whose own-turn content is missing would silently truncate the
    /// session on the next load.
    fn rewrite_episode(&mut self, episode: &Episode, own: &[Turn]) -> StoreResult<()>;

    /// Load one episode: its reconstructed [`Episode`] plus its turns in order.
    /// Errors with [`StoreError::EpisodeNotFound`] for an unknown id.
    fn load_episode(&self, id: &EpisodeId) -> StoreResult<LoadedEpisode>;

    /// Every episode id the store holds, in unspecified order.
    fn list_episodes(&self) -> StoreResult<Vec<EpisodeId>>;

    /// Remove an episode's log entirely — the destructive counterpart to
    /// [`put_episode`](Self::put_episode). **Idempotent**: deleting an unknown id
    /// is a no-op, not an error, so a double-delete is harmless. Refs that
    /// pointed at it are deliberately *not* touched — ref policy is the caller's
    /// (the App repoints `HEAD` to a fresh session). Its only caller is the
    /// `:delete` command, which gates it behind a confirmation.
    fn delete_episode(&mut self, id: &EpisodeId) -> StoreResult<()>;

    /// Point a named ref (e.g. `"HEAD"`, `"main"`) at an episode, overwriting any
    /// previous target. The name is validated by `validate_ref_name`.
    fn put_ref(&mut self, name: &str, episode: &EpisodeId) -> StoreResult<()>;

    /// Resolve a named ref to its episode id, or `None` if unset. The name is
    /// validated by `validate_ref_name`.
    fn get_ref(&self, name: &str) -> StoreResult<Option<EpisodeId>>;

    /// Rebuild the whole in-memory [`Graph`] by scanning every episode — the
    /// "graph.json rebuilt at startup" path (no at-rest cache in M3). Provided
    /// in terms of the primitives, so a backend gets it for free; a backend with
    /// a faster bulk path may override it.
    ///
    /// A single unloadable episode (corruption, a stray file) is **skipped with
    /// a warning**, not propagated — one bad file must not deny the user every
    /// other episode. A failure to even *list* episodes is still fatal.
    fn load_graph(&self) -> StoreResult<Graph> {
        let mut graph = Graph::new();
        for id in self.list_episodes()? {
            match self.load_episode(&id) {
                Ok(loaded) => {
                    // Content only: each record's provenance is already in the
                    // episode, as the occurrence its `Own` segment holds.
                    for turn in loaded.turns {
                        graph.insert_content(turn);
                    }
                    graph.insert_episode(loaded.episode);
                }
                Err(e) => {
                    warn!(episode = %id.to_hex(), error = %e, "skipping unloadable episode");
                }
            }
        }
        Ok(graph)
    }

    /// The graph's **structure** — every episode with its occurrences, and each
    /// turn's [`outline`](crate::Graph::outline) — with message bodies loaded
    /// only where that costs nothing extra. What a launch and every graph view
    /// read: enough to list, label, inspect and compose sessions; a reader of
    /// the words calls [`hydrate`] first.
    ///
    /// A backend that cannot load less (JSONL reads whole lines) answers with
    /// [`load_graph`](Self::load_graph), and is correct. A backend that can
    /// (SQLite: bodies in their own table) loads less, with
    /// [`load_bodies`](Self::load_bodies) to match.
    ///
    /// **Required, with no default** — this and the next two. A provided
    /// method is one a wrapper can forget to forward, and the wrapper then
    /// answers with the default instead of its backend's, compiling and
    /// passing: `FailingStore` did exactly that, and only the commit that
    /// added this pair noticed (Reviewers C and D).
    fn load_structure(&self) -> StoreResult<Graph>;

    /// The message bodies of `ids`, for the turns that have one stored; an id
    /// the store does not hold is left out. A full-loading backend answers
    /// with [`bodies_from_full_load`].
    fn load_bodies(&self, ids: &[TurnId]) -> StoreResult<Vec<(TurnId, Vec<Message>)>>;

    /// How many of `id`'s own turns the store holds — `None` when it holds no
    /// such episode. **What every "is it on disk?" asks**: a persist cursor
    /// resolves from it, and a guard that needs only existence asks it rather
    /// than loading the episode, which on a lazy store reads every own body.
    /// An `Err` means the store could not tell — never "absent": registering a
    /// session that is in fact on disk duplicates its header, and the
    /// registration's rollback deletes the log.
    fn episode_len(&self, id: &EpisodeId) -> StoreResult<Option<usize>>;
}

/// [`Store::load_bodies`] for a backend that can only load everything: one
/// full load, and the asked-for bodies out of it. **Asking for nothing reads
/// nothing** — the hydrate door asks only for what is missing, but a full
/// read for an empty answer would cost every reading verb a whole-history
/// parse on the store that ships.
pub fn bodies_from_full_load(
    store: &dyn Store,
    ids: &[TurnId],
) -> StoreResult<Vec<(TurnId, Vec<Message>)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let graph = store.load_graph()?;
    Ok(ids
        .iter()
        .filter_map(|id| Some((*id, graph.messages(id)?.to_vec())))
        .collect())
}

/// [`Store::episode_len`] for a backend whose only way to know is to load the
/// episode. Absence is `None`; every other failure is the store's, and says so.
pub fn episode_len_by_loading(store: &dyn Store, id: &EpisodeId) -> StoreResult<Option<usize>> {
    match store.load_episode(id) {
        Ok(loaded) => Ok(Some(loaded.episode.own_len())),
        Err(StoreError::EpisodeNotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// **Load the bodies `episodes` will be read for** — every turn of their
/// derived sequences whose body `graph` does not hold — so `Graph::records`
/// and `Graph::messages` answer for them. A path that reads turns and skipped
/// it would show a shorter session than the one on disk.
///
/// Turns the store does not hold stay unloaded; the reader's `records` then
/// says `None`, and it says so, rather than skip them.
pub fn hydrate(store: &dyn Store, graph: &mut Graph, episodes: &[EpisodeId]) -> StoreResult<()> {
    let wanted: Vec<TurnId> = episodes.iter().flat_map(|e| graph.full_turns(e)).collect();
    hydrate_turns(store, graph, &wanted)
}

/// **The one door between a structure-only graph and a reader of the words**:
/// load the bodies of `turns` that `graph` does not hold — none, and the store
/// is not asked at all. [`hydrate`] uses it for whole sessions; the inspector
/// for the one message it shows.
pub fn hydrate_turns(store: &dyn Store, graph: &mut Graph, turns: &[TurnId]) -> StoreResult<()> {
    let missing = graph.unloaded(turns.iter().copied());
    if !missing.is_empty() {
        graph.fill_bodies(store.load_bodies(&missing)?);
    }
    Ok(())
}

/// Reject ref names that are unsafe as a flat `refs/<name>` filename: the name
/// must be non-empty, not `.`/`..`, and contain only `[A-Za-z0-9._-]` — no path
/// separators, whitespace, or control chars. Banning `/` already forecloses
/// path traversal; the `.`/`..` checks are belt-and-suspenders. Enforced for
/// **every** backend (not just JSONL) so ref semantics never depend on the
/// storage choice. Hierarchical refs (`refs/heads/main`) are a future extension.
pub(crate) fn validate_ref_name(name: &str) -> StoreResult<()> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(StoreError::InvalidRef(name.to_string()))
    }
}

/// The on-disk store root for the project at `project_path`, under `state_dir`
/// (typically `…/shannon` from [`resolve_state_dir`](crate::config::resolve_state_dir)):
/// `<state_dir>/projects/<id>/`, where `<id>` is a short BLAKE3 of the project's
/// path. Each working directory thus gets its own isolated history (design.md
/// §1.5's `projects/<project-id>/`) — relaunch in the same dir, resume that
/// dir's conversation. Pure (no env / canonicalization) so it's testable; the
/// caller resolves `state_dir` and the launch directory.
pub fn project_store_root(state_dir: &Path, project_path: &Path) -> PathBuf {
    state_dir.join("projects").join(project_id(project_path))
}

/// **The project's id** — 16 hex of a BLAKE3 of its path: the history store's
/// directory ([`project_store_root`]), and the hash in its readable name
/// ([`project_name`]). One derivation, so the two cannot disagree.
fn project_id(project_path: &Path) -> String {
    let mut h = blake3::Hasher::new();
    // The pre-rename spelling, on purpose: every launch recomputes a project's
    // directory from this hash, so changing it orphans every stored session
    // (`the_project_id_derivation_never_changes`).
    h.update(b"shanon.project\0");
    h.update(project_path.as_os_str().as_encoded_bytes());
    h.finalize().to_hex()[..16].to_string()
}

/// **The project's readable name** — its directory's own name and the first 8
/// hex of the store's own id (`project_id`): `my-project-25ee4b3b`. What `{project}` stands for
/// in a memory or skill location (Steven, 10-06: "with a readable name"), so
/// a directory of them can be browsed by hand. The hash keeps two projects of
/// one name apart; a name that is not a plain word (`/`, a dot-name, odd
/// bytes) keeps only its safe characters.
pub fn project_name(project_path: &Path) -> String {
    let base: String = project_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    let base = base.trim_start_matches('.');
    let hash = &project_id(project_path)[..8];
    match base.is_empty() {
        true => hash.to_string(),
        false => format!("{base}-{hash}"),
    }
}

#[cfg(test)]
mod path_tests {
    use super::project_store_root;
    use std::path::{Path, PathBuf};

    #[test]
    fn project_root_is_under_projects_and_path_dependent() {
        let state = Path::new("/state/shannon");
        let a = project_store_root(state, Path::new("/work/alpha"));
        let b = project_store_root(state, Path::new("/work/beta"));
        assert!(a.starts_with(PathBuf::from("/state/shannon/projects")));
        assert_ne!(a, b, "different projects get different roots");
        assert_eq!(
            a,
            project_store_root(state, Path::new("/work/alpha")),
            "stable"
        );
        // The id segment is the 16-char hash prefix.
        let id = a.file_name().unwrap().to_string_lossy();
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// **The derivation is persistent identity.** Every launch recomputes a
    /// project's directory from its path, so a change to this hash — its
    /// domain separator's spelling included — orphans every stored session.
    #[test]
    fn the_project_id_derivation_never_changes() {
        assert_eq!(
            project_store_root(Path::new("/state"), Path::new("/work/alpha")),
            PathBuf::from("/state/projects/25ee4b3b9f89bb02")
        );
    }

    /// **A readable name, the store's hash in it** — so two projects named
    /// alike stay apart, and a name that is no plain word cannot climb.
    #[test]
    fn a_project_name_is_readable_and_its_hash_is_the_stores() {
        use super::project_name;
        assert_eq!(project_name(Path::new("/work/alpha")), "alpha-25ee4b3b");
        assert_ne!(
            project_name(Path::new("/elsewhere/alpha")),
            project_name(Path::new("/work/alpha"))
        );
        assert_eq!(project_name(Path::new("/")).len(), 8, "no name: the hash");
        for odd in ["/w/../..", "/w/.hidden", "/w/a b", "/w/x\u{0}y"] {
            let name = project_name(Path::new(odd));
            assert!(
                !name.contains(['/', ' ']) && !name.starts_with('.'),
                "{odd}: {name}"
            );
        }
    }
}

/// The shared backend contract: every assertion here must hold for **every**
/// store — `MemoryStore`, `JsonlStore`, and the `LazyBodies` and
/// `FailingStore` wrappers. Each one's test module wires them all with
/// `contract_tests!(<a fresh store>)`, one `#[test]` per function (so failures
/// localize per backend); this module is the single source of truth for what
/// a `Store` *means*. Adding a backend = one `contract_tests!` line, then make
/// these pass.
#[cfg(test)]
pub(crate) mod contract {
    use super::*;
    use crate::graph::{EdgeKind, Node, Origin, TurnConfig};
    use crate::message::Message;
    use std::time::SystemTime;

    // Shared by the contract assertions and each backend's own extra tests.
    pub(crate) fn cfg() -> TurnConfig {
        TurnConfig {
            model: "claude-haiku".into(),
            max_tokens: None,
            effort: None,
        }
    }

    fn ep_id(seed: &[u8]) -> EpisodeId {
        EpisodeId::from_seed(seed)
    }

    /// A 2-message turn keyed off `tag`; distinct tags give distinct ids.
    pub(crate) fn turn(tag: &str) -> Turn {
        Turn::seal(
            vec![Message::user(tag), Message::assistant(tag)],
            cfg(),
            SystemTime::UNIX_EPOCH,
        )
    }

    pub(crate) fn roundtrips_episode_metadata(s: &mut impl Store) {
        let id = ep_id(b"root");
        s.put_episode(&Episode::root(id, cfg())).unwrap();

        let loaded = s.load_episode(&id).unwrap();
        assert_eq!(loaded.episode.id, id);
        assert_eq!(loaded.episode.config, cfg());
        assert_eq!(loaded.episode.origin, None);
        assert!(loaded.episode.own_is_empty(), "no turns appended yet");
        assert!(loaded.turns.is_empty());
        assert!(loaded.episode.label.is_empty(), "no name was given");
    }

    /// **A session's name and description are kept** — by its first header,
    /// changed by a re-put once turns follow (the last header wins and adds
    /// no turn), read by every load, and kept by a rewrite. A rename is a
    /// re-put, so a store that dropped the label at any one of these would
    /// lose a name on the path that rename, `:graph` or a squash takes.
    pub(crate) fn a_label_is_kept_and_a_reput_changes_it(s: &mut impl Store) {
        let id = ep_id(b"named");
        let mut episode = Episode::root(id, cfg());
        episode.label = crate::graph::Label {
            name: Some("auth-fix".into()),
            description: None,
        };
        s.put_episode(&episode).unwrap();
        let a = turn("a");
        s.append_turn(&id, &a).unwrap();
        assert_eq!(s.load_episode(&id).unwrap().episode.label, episode.label);

        episode.label.description = Some("the token refresh race".into());
        s.put_episode(&episode).unwrap();
        let loaded = s.load_episode(&id).unwrap();
        assert_eq!(loaded.episode.label, episode.label, "re-put");
        assert_eq!(
            loaded.turns,
            vec![a.clone()],
            "a re-put adds and drops no turn"
        );
        for (how, graph) in [
            ("graph", s.load_graph().unwrap()),
            ("structure", s.load_structure().unwrap()),
        ] {
            let label = graph.episode(&id).map(|e| e.label.clone());
            assert_eq!(label, Some(episode.label.clone()), "{how}");
        }

        let loaded = s.load_episode(&id).unwrap().episode;
        s.rewrite_episode(&loaded, &[a]).unwrap();
        assert_eq!(
            s.load_episode(&id).unwrap().episode.label,
            episode.label,
            "rewritten"
        );
    }

    /// **Where an import came from is kept** — the Claude Code conversation
    /// an imported session continues ([`Inherited`](crate::graph::Inherited)),
    /// by every path a label takes THROUGH A STORE: the first header, a
    /// re-put, every load, a rewrite of what it holds. A store that dropped it
    /// on any one would turn a continued conversation into one born here,
    /// which is what a delete of Shannon's own sessions would then take. The
    /// graph's own rebuild, which a turn delete writes, is the graph's to
    /// keep (`a_rewrite_keeps_the_sessions_name_and_import_record`): this
    /// rewrites the store's loaded episode, and passed while that rebuild
    /// dropped it (review stop 2, A F1).
    pub(crate) fn an_import_record_is_kept(s: &mut impl Store) {
        let id = ep_id(b"imported");
        let mut episode = Episode::root(id, cfg());
        let from = crate::graph::Inherited {
            session: "abc12345-0000-0000-0000-000000000000".into(),
            turns: 3,
            // Where it parted, kept by every store as the rest is.
            at: Some("2b3c4d5e-entry".into()),
            opened: Vec::new(),
        };
        episode.inherited = Some(from.clone());
        s.put_episode(&episode).unwrap();
        let a = turn("a");
        s.append_turn(&id, &a).unwrap();
        let read = |s: &dyn Store, id| s.load_episode(&id).unwrap().episode.inherited;
        assert_eq!(read(s, id), Some(from.clone()), "first header");

        episode.label.name = Some("renamed".into());
        s.put_episode(&episode).unwrap();
        assert_eq!(read(s, id), Some(from.clone()), "re-put");
        for (how, graph) in [
            ("graph", s.load_graph().unwrap()),
            ("structure", s.load_structure().unwrap()),
        ] {
            let kept = graph.episode(&id).and_then(|e| e.inherited.clone());
            assert_eq!(kept, Some(from.clone()), "{how}");
        }
        let loaded = s.load_episode(&id).unwrap().episode;
        s.rewrite_episode(&loaded, &[a]).unwrap();
        assert_eq!(read(s, id), Some(from), "rewritten");

        let born = ep_id(b"born here");
        s.put_episode(&Episode::root(born, cfg())).unwrap();
        assert_eq!(read(s, born), None, "and a session born here has none");
    }

    /// **A session's marks are kept** ([`Episode::marks`]) by every path its
    /// header takes through a store — the first, a re-put (a rename), both
    /// loads of the graph, a rewrite — yours and the agent's apart, the
    /// agent's note with it (Steven, 10-28: "agent marks shouldn't be
    /// destroyed when we exit").
    pub(crate) fn marks_are_kept(s: &mut impl Store) {
        use crate::graph::{Mark, Whose};
        let id = ep_id(b"marked");
        let mut episode = Episode::root(id, cfg());
        let mark = |name, whose, note: &str| Mark {
            name,
            whose,
            turn: 0,
            block: "agent".into(),
            nth: 0,
            line: 1,
            col: 2,
            note: note.into(),
        };
        let marks = vec![
            mark('a', Whose::Yours, ""),
            mark('a', Whose::Agents, "the plan"),
        ];
        // And one saved entry this build does not read, kept with them
        // (review stop 14, B F4).
        let unread = vec![serde_json::json!({ "name": "c", "whose": "shared", "turn": 0 })];
        episode.marks = Some(marks.clone());
        episode.unread_marks = unread.clone();
        s.put_episode(&episode).unwrap();
        let a = turn("a");
        s.append_turn(&id, &a).unwrap();
        let read = |s: &dyn Store, id| {
            let episode = s.load_episode(&id).unwrap().episode;
            (episode.marks, episode.unread_marks)
        };
        let both = (Some(marks.clone()), unread.clone());
        assert_eq!(read(s, id), both, "first header");
        episode.label.name = Some("renamed".into());
        s.put_episode(&episode).unwrap();
        assert_eq!(read(s, id), both, "re-put");
        // A header without them — an older build's, or a put that does not
        // carry them — keeps the last, in every store (review stop 14, A 10:
        // the memory store cleared them), what it did not read with them.
        let mut bare = episode.clone();
        bare.marks = None;
        bare.unread_marks = Vec::new();
        s.put_episode(&bare).unwrap();
        assert_eq!(read(s, id), both, "a header without them");
        for (how, graph) in [
            ("graph", s.load_graph().unwrap()),
            ("structure", s.load_structure().unwrap()),
        ] {
            let kept = (graph.episode(&id)).map(|e| (e.marks.clone(), e.unread_marks.clone()));
            assert_eq!(kept, Some(both.clone()), "{how}");
        }
        let loaded = s.load_episode(&id).unwrap().episode;
        s.rewrite_episode(&loaded, &[a]).unwrap();
        assert_eq!(read(s, id), both, "rewritten");
        // Yours all gone: said, and what this build did not read still kept.
        episode.marks = Some(Vec::new());
        s.put_episode(&episode).unwrap();
        assert_eq!(read(s, id), (Some(Vec::new()), unread), "all gone, said");
        let born = ep_id(b"unmarked");
        s.put_episode(&Episode::root(born, cfg())).unwrap();
        assert_eq!(
            read(s, born),
            (None, Vec::new()),
            "and a session never marked has none"
        );
    }

    pub(crate) fn appends_turns_in_order(s: &mut impl Store) {
        let id = ep_id(b"e");
        s.put_episode(&Episode::root(id, cfg())).unwrap();
        let (a, b, c) = (turn("a"), turn("b"), turn("c"));
        for t in [&a, &b, &c] {
            s.append_turn(&id, t).unwrap();
        }

        let loaded = s.load_episode(&id).unwrap();
        assert_eq!(
            loaded.episode.own_turns(),
            vec![a.id, b.id, c.id],
            "order preserved"
        );
        assert_eq!(loaded.turns, vec![a.clone(), b.clone(), c.clone()]);
        assert_eq!(loaded.episode.own_head(), Some(c.id));
    }

    pub(crate) fn load_missing_episode_is_error(s: &mut impl Store) {
        let err = s.load_episode(&ep_id(b"ghost")).unwrap_err();
        assert!(matches!(err, StoreError::EpisodeNotFound(_)), "got {err:?}");
    }

    pub(crate) fn append_to_missing_episode_is_error(s: &mut impl Store) {
        let err = s.append_turn(&ep_id(b"ghost"), &turn("x")).unwrap_err();
        assert!(matches!(err, StoreError::EpisodeNotFound(_)), "got {err:?}");
    }

    pub(crate) fn refs_put_get_and_overwrite(s: &mut impl Store) {
        assert_eq!(s.get_ref("HEAD").unwrap(), None, "unset ref is None");
        let (a, b) = (ep_id(b"a"), ep_id(b"b"));
        s.put_ref("HEAD", &a).unwrap();
        assert_eq!(s.get_ref("HEAD").unwrap(), Some(a));
        s.put_ref("HEAD", &b).unwrap();
        assert_eq!(s.get_ref("HEAD").unwrap(), Some(b), "ref overwrites");
    }

    pub(crate) fn rejects_unsafe_ref_names(s: &mut impl Store) {
        let id = ep_id(b"x");
        for bad in ["", ".", "..", "a/b", "../evil", "a\\b", "a b"] {
            assert!(
                matches!(s.put_ref(bad, &id), Err(StoreError::InvalidRef(_))),
                "put_ref accepted unsafe name {bad:?}"
            );
            assert!(
                matches!(s.get_ref(bad), Err(StoreError::InvalidRef(_))),
                "get_ref accepted unsafe name {bad:?}"
            );
        }
    }

    pub(crate) fn lists_all_episodes(s: &mut impl Store) {
        let (a, b) = (ep_id(b"a"), ep_id(b"b"));
        s.put_episode(&Episode::root(a, cfg())).unwrap();
        s.put_episode(&Episode::root(b, cfg())).unwrap();

        let mut got = s.list_episodes().unwrap();
        got.sort_by(|x, y| x.as_bytes().cmp(y.as_bytes()));
        let mut want = vec![a, b];
        want.sort_by(|x, y| x.as_bytes().cmp(y.as_bytes()));
        assert_eq!(got, want);
    }

    pub(crate) fn delete_removes_an_episode_and_is_idempotent(s: &mut impl Store) {
        let id = ep_id(b"doomed");
        s.put_episode(&Episode::root(id, cfg())).unwrap();
        s.append_turn(&id, &turn("a")).unwrap();
        assert!(s.load_episode(&id).is_ok(), "present before delete");

        s.delete_episode(&id).unwrap();
        assert!(
            matches!(
                s.load_episode(&id).unwrap_err(),
                StoreError::EpisodeNotFound(_)
            ),
            "gone after delete"
        );
        assert!(
            !s.list_episodes().unwrap().contains(&id),
            "no longer listed"
        );

        // Idempotent: re-deleting, or deleting an unknown id, is a no-op.
        s.delete_episode(&id).unwrap();
        s.delete_episode(&ep_id(b"never-existed")).unwrap();
    }

    pub(crate) fn branch_episode_preserves_its_origin(s: &mut impl Store) {
        let (root, child) = (ep_id(b"root"), ep_id(b"child"));
        s.put_episode(&Episode::root(root, cfg())).unwrap();
        let a = turn("a");
        s.append_turn(&root, &a).unwrap();

        let origin = Origin {
            episode: root,
            turn: a.id,
            index: Some(0),
            kind: EdgeKind::Branch,
        };
        s.put_episode(&Episode::branch(child, cfg(), origin, 1))
            .unwrap();
        s.append_slice(&child, &root, 0, 1).unwrap(); // the shared prefix record

        let loaded = s.load_episode(&child).unwrap();
        assert_eq!(loaded.episode.origin, Some(origin), "origin survives");
    }

    /// The piece-table invariant on disk: a branch's log stores **only its
    /// divergent turns** plus a small `Slice` record; the shared prefix is never
    /// copied. The prefix is reconstructed by resolving that slice through
    /// [`Graph::full_turns`](crate::Graph::full_turns) — so long histories cost
    /// `O(divergence)` on disk, not `O(history)` per branch (design.md §1.1).
    pub(crate) fn branch_persists_divergent_turns_only(s: &mut impl Store) {
        // Root a, b. A branch shares [a, b] (a Slice) and diverges with c.
        let root = ep_id(b"root");
        s.put_episode(&Episode::root(root, cfg())).unwrap();
        let (a, b, c) = (turn("a"), turn("b"), turn("c"));
        s.append_turn(&root, &a).unwrap();
        s.append_turn(&root, &b).unwrap();

        let child = ep_id(b"child");
        let origin = Origin {
            episode: root,
            turn: b.id,
            index: Some(1),
            kind: EdgeKind::Branch,
        };
        s.put_episode(&Episode::branch(child, cfg(), origin, 2))
            .unwrap();
        s.append_slice(&child, &root, 0, 2).unwrap(); // shares [a, b] by reference
        s.append_turn(&child, &c).unwrap(); // only the divergent turn

        // On disk the branch owns just [c] — the prefix is NOT copied.
        let loaded = s.load_episode(&child).unwrap();
        assert_eq!(
            loaded.episode.own_turns(),
            vec![c.id],
            "divergent turn only"
        );
        assert_eq!(loaded.turns, vec![c.clone()], "prefix not re-stored");

        // The full history reconstructs from the shared prefix + the divergence.
        let graph = s.load_graph().unwrap();
        assert_eq!(
            graph.full_turns(&child),
            vec![a.id, b.id, c.id],
            "derivation stitches the shared prefix back on"
        );
        assert_eq!(graph.head(&child), Some(c.id));
    }

    /// A re-root's log stores its new-root turns + one `Slice` for the shared
    /// suffix — the *suffix* twin of the branch invariant (design.md §1.2).
    pub(crate) fn re_root_persists_new_root_and_a_suffix_slice(s: &mut impl Store) {
        // Root a, b, c. A re-root grafts the suffix [b, c] onto a new root [x].
        let root = ep_id(b"root");
        s.put_episode(&Episode::root(root, cfg())).unwrap();
        let (a, b, c, x) = (turn("a"), turn("b"), turn("c"), turn("x"));
        for t in [&a, &b, &c] {
            s.append_turn(&root, t).unwrap();
        }

        let clone = ep_id(b"clone");
        let origin = Origin {
            episode: root,
            turn: b.id,
            index: Some(1),
            kind: EdgeKind::Clone,
        };
        s.put_episode(&Episode::from_segments(
            clone,
            cfg(),
            Vec::new(),
            Some(origin),
        ))
        .unwrap();
        s.append_turn(&clone, &x).unwrap(); // the new root (new data)
        s.append_slice(&clone, &root, 1, 3).unwrap(); // shares [b, c] by reference

        // On disk the clone owns just [x]; the suffix is a reference.
        let loaded = s.load_episode(&clone).unwrap();
        assert_eq!(loaded.episode.own_turns(), vec![x.id], "new root only");
        assert_eq!(loaded.turns, vec![x.clone()], "suffix not re-stored");

        let graph = s.load_graph().unwrap();
        assert_eq!(
            graph.full_turns(&clone),
            vec![x.id, b.id, c.id],
            "new root + shared suffix"
        );
        // One node, one place: the re-root nests under its origin turn after a
        // reload — not a second forest root — and displays its full history.
        assert!(!graph.roots().contains(&Node::Episode(clone)));
        assert!(graph.episode(&clone).unwrap().shows_full_history());
    }

    pub(crate) fn load_graph_rebuilds_the_dag(s: &mut impl Store) {
        // Root with turn A; a branch child forked at A with its own turn C. The
        // rebuilt graph must place the child under A via the one children step.
        let (root, child) = (ep_id(b"root"), ep_id(b"child"));
        s.put_episode(&Episode::root(root, cfg())).unwrap();
        let a = turn("a");
        s.append_turn(&root, &a).unwrap();

        let origin = Origin {
            episode: root,
            turn: a.id,
            index: Some(0),
            kind: EdgeKind::Branch,
        };
        s.put_episode(&Episode::branch(child, cfg(), origin, 1))
            .unwrap();
        s.append_slice(&child, &root, 0, 1).unwrap();
        let c = turn("c");
        s.append_turn(&child, &c).unwrap();

        let graph = s.load_graph().unwrap();
        assert_eq!(
            graph.roots(),
            vec![Node::Episode(root)],
            "only root is a forest root"
        );
        let at_a = graph.children(Node::Turn {
            episode: root,
            turn: a.id,
            index: 0,
        });
        assert!(
            at_a.contains(&Node::Episode(child)),
            "branch reachable through its origin turn"
        );
        assert_eq!(
            graph.messages(&a.id),
            Some(&a.messages[..]),
            "turn content survived"
        );
        assert_eq!(graph.messages(&c.id), Some(&c.messages[..]));
    }

    /// Two productions of one turn in one episode come back with their own
    /// facts — appended, through the graph, and rewritten. The rewrite is
    /// handed ONE record for the shared content: it needs content by id, never
    /// facts by id, which is how the JSONL rewrite once wrote the first
    /// production's provenance for both.
    ///
    /// The recorded secret handles are among those facts, in all three states
    /// recall tells apart: none held (`Some([])`, what a seal starts as), held
    /// (`Some([h])`), and legacy (`None` — the field absent, as a log older
    /// than it reads back). A backend that stored `None` and `[]` alike would
    /// put every new turn on the legacy scan.
    pub(crate) fn identical_turns_keep_their_own_provenance(s: &mut impl Store) {
        let id = ep_id(b"twice");
        s.put_episode(&Episode::root(id, cfg())).unwrap();
        let first = turn("hi");
        assert_eq!(first.provenance.handles, Some(vec![]), "premise: none held");
        let mut second = turn("hi").with_handles(vec!["{{pw.beef}}".into()]);
        second.provenance.at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(60);
        second.provenance.usage = Some(crate::Usage {
            output_tokens: 9,
            ..Default::default()
        });
        let mut third = turn("hi");
        third.provenance.handles = None;
        assert_eq!(first.id, second.id, "premise: one content");
        assert_eq!(first.id, third.id, "premise: one content");
        s.append_turn(&id, &first).unwrap();
        s.append_turn(&id, &second).unwrap();
        s.append_turn(&id, &third).unwrap();

        let all = vec![first.clone(), second, third];
        assert_eq!(s.load_episode(&id).unwrap().turns, all, "appended");
        assert_eq!(
            s.load_graph().unwrap().records(&id),
            Some(all.clone()),
            "through the graph"
        );

        let episode = s.load_episode(&id).unwrap().episode;
        s.rewrite_episode(&episode, &[first]).unwrap();
        assert_eq!(s.load_episode(&id).unwrap().turns, all, "rewritten");
    }

    /// `episode_len` answers "is it on disk, and how much of its own" — `None`
    /// for absence, the OWN count otherwise (a slice adds none: shared history
    /// is its owner's to count).
    pub(crate) fn episode_len_counts_own_turns(s: &mut impl Store) {
        let id = ep_id(b"len");
        assert_eq!(s.episode_len(&id).unwrap(), None, "absent");
        s.put_episode(&Episode::root(id, cfg())).unwrap();
        assert_eq!(
            s.episode_len(&id).unwrap(),
            Some(0),
            "registered, nothing owned"
        );
        s.append_turn(&id, &turn("a")).unwrap();
        s.append_turn(&id, &turn("b")).unwrap();
        let src = ep_id(b"len-src");
        s.put_episode(&Episode::root(src, cfg())).unwrap();
        s.append_turn(&src, &turn("c")).unwrap();
        s.append_slice(&id, &src, 0, 1).unwrap();
        assert_eq!(s.episode_len(&id).unwrap(), Some(2), "own turns only");
    }

    /// **The lazy pair answers what a full load does.** A structure load
    /// hydrated for every episode holds the same records as `load_graph`, and
    /// `load_bodies` leaves out an id it does not hold — the contract a lazy
    /// backend (SQLite) must meet to stand in for the one that loads it all.
    pub(crate) fn the_lazy_pair_matches_a_full_load(s: &mut impl Store) {
        let root = ep_id(b"lazy-root");
        s.put_episode(&Episode::root(root, cfg())).unwrap();
        s.append_turn(&root, &turn("x")).unwrap();
        s.append_turn(&root, &turn("y")).unwrap();
        // A session that shares the root's first turn by reference, then owns
        // one: the derived shape every composition writes.
        let branch = ep_id(b"lazy-branch");
        s.put_episode(&Episode::root(branch, cfg())).unwrap();
        s.append_slice(&branch, &root, 0, 1).unwrap();
        s.append_turn(&branch, &turn("z")).unwrap();

        let full = s.load_graph().unwrap();
        let mut lazy = s.load_structure().unwrap();
        let all = lazy.episode_ids();
        hydrate(&*s, &mut lazy, &all).unwrap();
        for id in [root, branch] {
            assert!(
                full.records(&id).is_some(),
                "premise: a full load holds {id:?}"
            );
            assert_eq!(lazy.records(&id), full.records(&id), "{id:?}");
        }
        let unknown = turn("never stored").id;
        let known = turn("x").id;
        let got: Vec<TurnId> = s
            .load_bodies(&[unknown, known])
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(got, [known], "an unknown id is left out");
    }

    /// **A label loads as one a person could have given**, by every load —
    /// whatever was put, the way the JSONL store reads a header a hand may
    /// have edited (`Label::read_back`): a spaced name is dropped, a
    /// description loses what no one could type, the session intact (review
    /// of 34033e8, Reviewer B). `MemoryStore` gave back what was put
    /// (review stop 12, T9 7).
    pub(crate) fn a_label_loads_as_one_a_person_could_give(s: &mut impl Store) {
        let id = ep_id(b"edited");
        let mut episode = Episode::root(id, cfg());
        episode.label.name = Some("two words".into());
        episode.label.description = Some("a\u{202e}b".into());
        s.put_episode(&episode).unwrap();
        let read_back = crate::graph::Label {
            name: None,
            description: Some("ab".into()),
        };
        assert_eq!(s.load_episode(&id).unwrap().episode.label, read_back);
        for (how, graph) in [
            ("graph", s.load_graph().unwrap()),
            ("structure", s.load_structure().unwrap()),
        ] {
            let label = graph.episode(&id).map(|e| e.label.clone());
            assert_eq!(label, Some(read_back.clone()), "{how}");
        }
    }

    /// A turn holding a call whose input nests `depth` deep — after brackets
    /// in a string that follow an escaped quote, which are no structure:
    /// counted, they would lower the depth.
    pub(crate) fn nested_turn(depth: usize) -> Turn {
        use crate::message::{Block, Role};
        let mut input = serde_json::json!(1);
        for _ in 0..depth {
            input = serde_json::json!([input]);
        }
        let input = serde_json::json!([format!("\"{}", "]".repeat(40)), input]);
        let call = Message {
            role: Role::Assistant,
            content: String::new(),
            blocks: vec![Block::ToolUse {
                id: "t".into(),
                name: "x".into(),
                input,
            }],
        };
        Turn::seal(
            vec![Message::user("q"), call],
            cfg(),
            SystemTime::UNIX_EPOCH,
        )
    }

    /// **What a store keeps, it reads back** — at every depth around the
    /// reader's limit, a turn holding a call nested that deep is refused by
    /// both writers (an append, a rewrite — the original left as it was) or
    /// loads (review stop 4, E 3: a 122-deep input imported, was written,
    /// then failed its session's load). Both happen in the range, so neither
    /// side of the limit is assumed.
    pub(crate) fn a_turn_too_deep_to_read_back_is_refused_at_the_write(s: &mut impl Store) {
        let (mut written, mut refused) = (0, 0);
        for depth in 110..140 {
            let deep = nested_turn(depth);
            let id = ep_id(format!("deep {depth}").as_bytes());
            s.put_episode(&Episode::root(id, cfg())).unwrap();
            let mut holding = Episode::root(id, cfg());
            holding.append(deep.occurrence());
            let rewrite = s.rewrite_episode(&holding, std::slice::from_ref(&deep));
            let after_rewrite = s.load_episode(&id).map(|l| l.turns.len());
            let append = s.append_turn(&id, &deep);
            match (rewrite, append) {
                (Ok(()), Ok(())) => {
                    assert_eq!(after_rewrite.ok(), Some(1), "rewritten at {depth}");
                    let loaded = s
                        .load_episode(&id)
                        .unwrap_or_else(|e| panic!("written at {depth}, not read back: {e}"));
                    assert_eq!(loaded.turns.len(), 2, "at {depth}");
                    written += 1;
                }
                (Err(StoreError::TooDeep { .. }), Err(StoreError::TooDeep { .. })) => {
                    assert_eq!(after_rewrite.ok(), Some(0), "left as it was at {depth}");
                    refused += 1;
                }
                (rewrite, append) => panic!("at {depth}: {rewrite:?} / {append:?}"),
            }
        }
        assert!(
            written > 0 && refused > 0,
            "premise, both sides: {written} written, {refused} refused"
        );
    }

    /// An episode holding `turns` as its own, in order — the shape a rewrite
    /// is handed.
    fn holding(id: EpisodeId, turns: &[&Turn]) -> Episode {
        let mut episode = Episode::root(id, cfg());
        for turn in turns {
            episode.append(turn.occurrence());
        }
        episode
    }

    /// **A rewrite takes a turn out** — what it is for (a turn deleted from
    /// the middle of a session, which no append can say). Of three, the
    /// first goes, and the third — its content the first's — stays as its
    /// own occurrence, its facts its own: a store that dropped by id would
    /// take both, or keep the first's facts. No contract rewrite had ever
    /// made a log shorter; keeping the old segments passed every store test
    /// (review stop 12, T9 2d).
    pub(crate) fn a_rewrite_takes_a_turn_out(s: &mut impl Store) {
        let id = ep_id(b"cut");
        s.put_episode(&Episode::root(id, cfg())).unwrap();
        let (first, second) = (turn("a"), turn("b"));
        let mut third = turn("a");
        third.provenance.at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(60);
        assert_eq!(first.id, third.id, "premise: one content");
        assert_ne!(first, third, "premise: two occurrences");
        for turn in [&first, &second, &third] {
            s.append_turn(&id, turn).unwrap();
        }
        let kept = holding(id, &[&second, &third]);
        s.rewrite_episode(&kept, &[second.clone(), third.clone()])
            .unwrap();
        let want = vec![second, third];
        assert_eq!(s.load_episode(&id).unwrap().turns, want);
        assert_eq!(s.load_graph().unwrap().records(&id), Some(want), "graph");
        assert_eq!(s.episode_len(&id).unwrap(), Some(2));
    }

    /// **A rewrite missing a turn's content is refused, and the log stays
    /// as it was** — written, it would load as a shorter session, data the
    /// user lost. The store double kept no such guard and every test passed
    /// (review stop 12, T9 2c).
    pub(crate) fn a_rewrite_missing_a_turn_is_refused(s: &mut impl Store) {
        let id = ep_id(b"missing");
        s.put_episode(&Episode::root(id, cfg())).unwrap();
        let (a, b) = (turn("a"), turn("b"));
        s.append_turn(&id, &a).unwrap();
        s.append_turn(&id, &b).unwrap();
        let said = s.rewrite_episode(&holding(id, &[&a, &b]), std::slice::from_ref(&a));
        assert!(matches!(said, Err(StoreError::Corrupt { .. })), "{said:?}");
        assert_eq!(s.load_episode(&id).unwrap().turns, vec![a, b]);
    }

    /// **A rewrite that fails part-way leaves the log as it was** — its
    /// turns, not an empty shell: the second turn of the new log is one the
    /// store cannot read back, so the write fails after the first. The
    /// depth test's log held no turns, so a rewrite in place passed it
    /// (review stop 12, T9 2a).
    pub(crate) fn a_rewrite_that_fails_leaves_the_log(s: &mut impl Store) {
        let id = ep_id(b"atomic");
        s.put_episode(&Episode::root(id, cfg())).unwrap();
        let (a, b, deep) = (turn("a"), turn("b"), nested_turn(140));
        s.append_turn(&id, &a).unwrap();
        s.append_turn(&id, &b).unwrap();
        let said = s.rewrite_episode(&holding(id, &[&a, &deep]), &[a.clone(), deep.clone()]);
        assert!(matches!(said, Err(StoreError::TooDeep { .. })), "{said:?}");
        assert_eq!(s.load_episode(&id).unwrap().turns, vec![a, b]);
    }

    /// **The contract, as one `#[test]` per function over the store `$make`
    /// builds** (a fresh one each). One list for every backend and wrapper,
    /// so each holds all of the contract: by hand, `LazyBodies` held 3 of 17
    /// under a comment saying "the whole contract still holds", and
    /// `FailingStore` none (review stop 12, T9 7). The list is held to the
    /// functions above by `the_suite_lists_every_contract_function`.
    macro_rules! contract_tests {
        // Every contract function, once; `@tests` and `@names` are fed it.
        // Braces, not `(…);`: the call stands as items for `@tests` and as
        // an expression for `@names`.
        (@list $arm:ident $($arg:tt)*) => {
            $crate::store::contract::contract_tests! { @$arm ($($arg)*)
                roundtrips_episode_metadata,
                a_label_is_kept_and_a_reput_changes_it,
                a_label_loads_as_one_a_person_could_give,
                an_import_record_is_kept,
                marks_are_kept,
                appends_turns_in_order,
                load_missing_episode_is_error,
                append_to_missing_episode_is_error,
                refs_put_get_and_overwrite,
                rejects_unsafe_ref_names,
                lists_all_episodes,
                delete_removes_an_episode_and_is_idempotent,
                branch_episode_preserves_its_origin,
                branch_persists_divergent_turns_only,
                re_root_persists_new_root_and_a_suffix_slice,
                load_graph_rebuilds_the_dag,
                identical_turns_keep_their_own_provenance,
                episode_len_counts_own_turns,
                the_lazy_pair_matches_a_full_load,
                a_turn_too_deep_to_read_back_is_refused_at_the_write,
                a_rewrite_takes_a_turn_out,
                a_rewrite_missing_a_turn_is_refused,
                a_rewrite_that_fails_leaves_the_log,
            }
        };
        (@tests ($make:expr) $($name:ident),* $(,)?) => {
            $(
                #[test]
                fn $name() {
                    $crate::store::contract::$name(&mut $make);
                }
            )*
        };
        (@names () $($name:ident),* $(,)?) => {
            [$(stringify!($name)),*]
        };
        ($make:expr) => {
            $crate::store::contract::contract_tests!(@list tests $make);
        };
    }
    pub(crate) use contract_tests;

    /// **The suite lists every contract function** — `contract_tests!`'s list
    /// is a second list of the functions here, so it is held to them: every
    /// `pub(crate) fn` of this module taking a store, read from this file.
    #[test]
    fn the_suite_lists_every_contract_function() {
        use std::collections::BTreeSet;
        let source = include_str!("mod.rs");
        let module = &source[source.find("pub(crate) mod contract").unwrap()..];
        let defined: BTreeSet<&str> = module
            .split("pub(crate) fn ")
            .skip(1)
            .filter_map(|rest| {
                let (name, signature) = rest.split_once('(')?;
                (signature.trim_start())
                    .starts_with("s: &mut impl Store")
                    .then_some(name)
            })
            .collect();
        let listed: BTreeSet<&str> = contract_tests!(@list names).into_iter().collect();
        assert!(
            defined.len() >= 19,
            "premise, the scan finds them: {defined:?}"
        );
        assert_eq!(listed, defined);
    }
}
