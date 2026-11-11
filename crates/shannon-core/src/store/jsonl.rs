//! [`JsonlStore`] — the on-disk [`Store`] backend, append-only and inspectable.
//!
//! The production format (design.md §1.3.1): one append-only JSON-Lines file per
//! episode, so the whole point survives — `cat`, `grep`, and `tail -f` still
//! work on your conversation history. Layout under the store root:
//!
//! ```text
//! <root>/episodes/<episode-id>.jsonl   # line 0: episode header; then one Turn per line
//! <root>/refs/<ref-name>               # a file holding an episode id (hex)
//! ```
//!
//! Each `.jsonl` line is one `Record`: an `episode` header (id + config +
//! parent edge + any name and description) written by
//! [`put_episode`](Store::put_episode), then a `turn`
//! record per [`append_turn`](Store::append_turn). The records are
//! self-describing (a `"kind"` tag) so a single line is meaningful on its own —
//! `tail -f` shows each new turn as it commits. The episode's *turn order* is
//! simply the order of turn lines; that is what makes an append a single line
//! write instead of a whole-file rewrite (the content-addressed split — see the
//! [`super`] module docs).
//!
//! There is **no `graph.json` at rest**: [`load_graph`](Store::load_graph)
//! rebuilds the DAG by scanning `episodes/` at startup. Loading also **verifies
//! each turn against its content hash**, turning silent bit-rot or a botched
//! hand-edit into an explicit [`StoreError::Corrupt`] rather than a wrong
//! transcript — which is what earns keeping the (otherwise redundant) id on disk.

use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::graph::{
    EdgeKind, Episode, EpisodeId, Inherited, Label, Origin, Segment, Turn, TurnConfig, TurnId,
};

use super::{LoadedEpisode, Store, StoreError, StoreResult, validate_ref_name};

/// A **legacy** pre-piece-table parent edge (episode-as-path era): a prefix-share
/// link carrying the shared-prefix length. Read-only — migrated on load into an
/// [`Origin`] plus a leading [`Segment::Slice`], never written. Lets sessions
/// created before the piece table keep loading.
#[derive(Deserialize)]
struct LegacyParentLink {
    episode: EpisodeId,
    turn: TurnId,
    fork_index: usize,
    kind: EdgeKind,
}

/// **A header field read as far as it goes** — its value, or why it did not
/// read. A field a hand edit or another build's shape broke drops alone, said
/// in the log ([`lenient`]), and the session loads without it: read strictly,
/// it failed the whole header and the session vanished, turns and all
/// (review stop 2, B 2 — the import record first, then the name and the
/// branch's origin, which had the same hole). Written as the value itself,
/// so a header's bytes are what they were.
#[derive(Debug, Clone)]
enum Lenient<T> {
    Read(T),
    Unread(String),
}

impl<T: Serialize> Serialize for Lenient<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Lenient::Read(value) => value.serialize(serializer),
            Lenient::Unread(_) => serializer.serialize_none(),
        }
    }
}

impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for Lenient<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        // **Why, in words that quote nothing**: serde's message does —
        // `invalid type: string "…"` — and a header holds a session's name
        // and description; the log is no place for them (review stop 4, E 7).
        let kind = match &value {
            serde_json::Value::Null => "null",
            serde_json::Value::Bool(_) => "boolean",
            serde_json::Value::Number(_) => "number",
            serde_json::Value::String(_) => "string",
            serde_json::Value::Array(_) => "list",
            serde_json::Value::Object(_) => "record",
        };
        Ok(match serde_json::from_value(value) {
            Ok(read) => Lenient::Read(read),
            Err(e) => Lenient::Unread(format!("a {kind} not of its shape ({:?})", e.classify())),
        })
    }
}

/// **A header field as the session takes it** — `None` when absent, and
/// when it did not read, which the log says, naming the session and `what`.
fn lenient<T>(id: &EpisodeId, what: &str, field: Option<Lenient<T>>) -> Option<T> {
    match field? {
        Lenient::Read(value) => Some(value),
        Lenient::Unread(e) => {
            warn!(
                episode = %id.to_hex(),
                error = %e,
                "{what} that does not read; the session loads without it",
            );
            None
        }
    }
}

/// One line of an episode `.jsonl` file. Internally tagged (`"kind"`) so each
/// line is self-describing and the set of record kinds can grow. The segment
/// sequence is built by replaying `turn` / `slice` records in order (a `turn`
/// extends the trailing `Own`; a `slice` shares a range of another episode by
/// reference) — the append-only twin of the in-memory piece table.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Record {
    /// Episode header: metadata only. The last header wins (a re-put appends a
    /// fresh header rather than rewriting), keeping the log append-only.
    Episode {
        id: EpisodeId,
        config: TurnConfig,
        /// Where a branch, clone or child hangs. One that does not read
        /// leaves the session a root — in the tree, if out of place.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<Lenient<Origin>>,
        /// A name and description, written only when a person gave one, so an
        /// unnamed session's header is the bytes it always was.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<Lenient<Label>>,
        /// The Claude Code conversation it was brought in from, written only
        /// for an import, so every other header is the bytes it always was.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        inherited: Option<Lenient<Inherited>>,
        /// Its marks ([`Episode::marks`]), written only once it has had one,
        /// so every other header is the bytes it always was. A header field,
        /// not a record of its own: an unknown key is ignored by an older
        /// build, where an unknown record kind on any line but the last fails
        /// the whole session. Read an entry at a time (`mark::read_saved`):
        /// one this build does not know is kept, not the list's ruin.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        marks: Option<Lenient<Vec<serde_json::Value>>>,
        /// Legacy pre-piece-table field, read-only (migrated on load). Never
        /// written by the current store (`skip_serializing`).
        #[serde(default, skip_serializing)]
        parent: Option<LegacyParentLink>,
    },
    /// A sealed turn appended to the episode's own turns, in commit order.
    Turn(Turn),
    /// A shared slice `[start, end)` of another episode's derived sequence — the
    /// structural-sharing record (a branch's prefix, a re-root's suffix). No turn
    /// content: the referenced turns live on `episode`.
    Slice {
        episode: EpisodeId,
        start: usize,
        end: usize,
    },
}

impl Record {
    /// **`episode`'s header** — the one shape both writers emit
    /// (`put_episode`, `rewrite_episode`), so a field the header gains is
    /// written by both or by neither.
    fn header(episode: &Episode) -> Record {
        Record::Episode {
            id: episode.id,
            config: episode.config.clone(),
            origin: episode.origin.map(Lenient::Read),
            label: (!episode.label.is_empty()).then(|| Lenient::Read(episode.label.clone())),
            inherited: episode.inherited.clone().map(Lenient::Read),
            // What it read and what it did not, so a save here keeps both.
            // No marks, no key: the last header's stand, unread and all.
            marks: episode.marks.as_deref().map(|marks| {
                Lenient::Read(crate::graph::mark::write_saved(
                    marks,
                    &episode.unread_marks,
                ))
            }),
            parent: None,
        }
    }
}

/// An on-disk [`Store`] rooted at a directory. Cheap to hold (just the path);
/// every operation touches the filesystem.
#[derive(Debug, Clone)]
pub struct JsonlStore {
    root: PathBuf,
}

impl JsonlStore {
    /// Open (creating if needed) a store rooted at `root`, ensuring the
    /// `episodes/` and `refs/` subdirectories exist.
    ///
    /// `root` is the caller's choice — production wires the XDG state dir; tests
    /// pass a throwaway `tempfile::TempDir` and never the real one.
    pub fn new(root: impl AsRef<Path>) -> StoreResult<Self> {
        let root = root.as_ref().to_path_buf();
        // Owner-only (0700): the state subtree holds conversation history, which
        // can contain pasted secrets / proprietary code / PII — same posture as
        // the log dir. The directories below `root` (projects/<hash>/…) are ours
        // to create; pre-existing parents (e.g. ~/.local/state) keep their perms.
        create_private_dir(&root.join("episodes"))?;
        create_private_dir(&root.join("refs"))?;
        Ok(Self { root })
    }

    fn episode_path(&self, id: &EpisodeId) -> PathBuf {
        self.root
            .join("episodes")
            .join(format!("{}.jsonl", id.to_hex()))
    }

    /// Append a record to an **already-registered** episode's log. A missing file
    /// means the episode was never `put_episode`'d → [`StoreError::EpisodeNotFound`]
    /// (a permission error stays `Io`). Race-free vs. a prior `exists()` check.
    fn append_existing(&self, episode: &EpisodeId, record: &Record) -> StoreResult<()> {
        match append_record(&self.episode_path(episode), record, false) {
            Err(StoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(StoreError::EpisodeNotFound(episode.to_hex()))
            }
            other => other,
        }
    }

    fn ref_path(&self, name: &str) -> PathBuf {
        self.root.join("refs").join(name)
    }
}

/// Create `path` (and any missing parents) restricted to the owner (0700) on
/// unix — matching the log dir's posture, since the state subtree holds
/// conversation history. Perms are a no-op on non-unix (not modeled there).
fn create_private_dir(path: &Path) -> StoreResult<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}

/// Restrict a file this `OpenOptions` will *create* to the owner (0600) on unix
/// — episode logs and refs hold conversation content, so they match the 0600
/// posture of editor tempfiles. Only the create path is affected; appending to
/// an existing file never loosens its perms. No-op on non-unix.
fn owner_only(opts: &mut fs::OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = opts;
}

/// **`record` as the line written for it — one the store can read back**,
/// or why not ([`check_readable`](super::check_readable)). The door both
/// writers take (`append_record`, and `rewrite_episode`'s `emit`): a turn
/// whose call input nested 122 deep in Claude Code's own line imported, was
/// written, and then failed its session's load, with every turn after it
/// (review stop 4, E 3).
fn line(record: &Record) -> StoreResult<String> {
    let mut line = serde_json::to_string(record)?;
    super::check_readable(&line)?;
    line.push('\n');
    Ok(line)
}

/// Append one record as a JSON line. With `create`, the file is created
/// owner-only if absent (used for the episode header); without it, a missing
/// file is an error (an `append_turn` to an unregistered episode). The `append`
/// open mode is what keeps the base log immutable-once-written.
fn append_record(path: &Path, record: &Record, create: bool) -> StoreResult<()> {
    let line = line(record)?;
    let mut opts = fs::OpenOptions::new();
    opts.read(true).append(true).create(create);
    owner_only(&mut opts);
    let mut file = opts.open(path)?;
    mend_the_tail(&mut file, path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// **The log ends where a record ends before another goes on it** (review
/// stop 12, T9 1). A crash or a full disk mid-append leaves a last line
/// with no newline, which the loader drops as torn while it is LAST. The
/// next record was written straight onto it: fused, it was dropped as torn
/// too, and the record after left the fused line in the middle, garbled —
/// the whole session failed to load and left `:graph`.
///
/// So, before appending: a file ending in a newline is left (one byte
/// read). A tail that parses as a record lacks only its newline, which is
/// written. One that does not is the torn write, cut back to the last whole
/// line — where the loader already read the file as ending, so nothing it
/// would have shown is lost. Not coordinated with another process appending
/// to the same log at the same moment: the store takes no lock anywhere.
fn mend_the_tail(file: &mut fs::File, path: &Path) -> StoreResult<()> {
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(());
    }
    let mut last = [0u8; 1];
    file.seek(SeekFrom::End(-1))?;
    file.read_exact(&mut last)?;
    if last[0] == b'\n' {
        return Ok(());
    }
    // Where the last line starts: back from the end, a block at a time, to
    // the newline before it (or the file's start).
    let mut block = vec![0u8; 64 * 1024];
    let (mut end, mut tail_at) = (len, 0);
    while end > 0 {
        let from = end.saturating_sub(block.len() as u64);
        let read = &mut block[..(end - from) as usize];
        file.seek(SeekFrom::Start(from))?;
        file.read_exact(read)?;
        if let Some(at) = read.iter().rposition(|&b| b == b'\n') {
            tail_at = from + at as u64 + 1;
            break;
        }
        end = from;
    }
    let mut tail = Vec::new();
    file.seek(SeekFrom::Start(tail_at))?;
    file.read_to_end(&mut tail)?;
    let whole = std::str::from_utf8(&tail).is_ok_and(|t| serde_json::from_str::<Record>(t).is_ok());
    if whole {
        file.write_all(b"\n")?;
    } else {
        warn!(
            path = %path.display(),
            bytes = tail.len(),
            "cutting a torn last line before appending (an incomplete write)",
        );
        file.set_len(tail_at)?;
    }
    Ok(())
}

impl Store for JsonlStore {
    fn put_episode(&mut self, episode: &Episode) -> StoreResult<()> {
        append_record(
            &self.episode_path(&episode.id),
            &Record::header(episode),
            true,
        )
    }

    fn rewrite_episode(&mut self, episode: &Episode, own: &[Turn]) -> StoreResult<()> {
        super::check_own_present(episode, own)?;
        // **Build the whole replacement first, then swap it in.** The log is
        // the segment sequence, so truncating in place would expose a
        // half-written history to any concurrent reader and would lose the
        // session outright if the process died mid-write. `rename` within the
        // same directory is atomic on POSIX: readers see the old file or the
        // new one, never a partial one, and a failure anywhere before it leaves
        // the original exactly as it was.
        let path = self.episode_path(&episode.id);
        let tmp = path.with_extension("jsonl.rewrite");
        // Owner-only like every other file we create: the replacement holds the
        // same conversation content as the log it replaces
        // ([[feedback_persisted_data_perms]]), and it is born with the content
        // already in it, so a default-permission window never exists.
        let mut opts = fs::OpenOptions::new();
        opts.create(true).write(true).truncate(true);
        owner_only(&mut opts);

        let write = |mut file: fs::File| -> StoreResult<()> {
            let mut emit = |record: &Record| -> StoreResult<()> {
                file.write_all(line(record)?.as_bytes())?;
                Ok(())
            };
            emit(&Record::header(episode))?;
            for segment in episode.segments() {
                match segment {
                    Segment::Own(occurrences) => {
                        for occurrence in occurrences {
                            // The CONTENT by id; the facts from this occurrence.
                            let content = own
                                .iter()
                                .find(|t| t.id == occurrence.turn)
                                .expect("checked by check_own_present");
                            emit(&Record::Turn(Turn::of(
                                occurrence,
                                content.messages.clone(),
                            )))?;
                        }
                    }
                    Segment::Slice {
                        episode: source,
                        start,
                        end,
                    } => emit(&Record::Slice {
                        episode: *source,
                        start: *start,
                        end: *end,
                    })?,
                }
            }
            // Flush before the rename, or the swap can publish a file whose
            // tail is still in a buffer.
            file.sync_all()?;
            Ok(())
        };

        match opts.open(&tmp).map_err(StoreError::from).and_then(write) {
            Ok(()) => {
                fs::rename(&tmp, &path)?;
                Ok(())
            }
            Err(e) => {
                // Best-effort cleanup; the original log is untouched either way.
                let _ = fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    fn append_turn(&mut self, episode: &EpisodeId, turn: &Turn) -> StoreResult<()> {
        // Don't create — a missing file means the episode was never registered.
        // Opening with `create(false)` and mapping `NotFound` is race-free,
        // unlike a prior `path.exists()` check (and a permission error now
        // surfaces as `Io`, not a misleading `EpisodeNotFound`).
        self.append_existing(episode, &Record::Turn(turn.clone()))
    }

    fn append_slice(
        &mut self,
        episode: &EpisodeId,
        source: &EpisodeId,
        start: usize,
        end: usize,
    ) -> StoreResult<()> {
        self.append_existing(
            episode,
            &Record::Slice {
                episode: *source,
                start,
                end,
            },
        )
    }

    fn load_episode(&self, id: &EpisodeId) -> StoreResult<LoadedEpisode> {
        let file = match fs::File::open(self.episode_path(id)) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::EpisodeNotFound(id.to_hex()));
            }
            Err(e) => return Err(e.into()),
        };

        let corrupt = |detail: String| StoreError::Corrupt {
            episode: id.to_hex(),
            detail,
        };

        // Last header wins; turn/slice records build the segments in order. Verify
        // each turn against its content hash so a corrupted line fails loudly.
        /// The last header's metadata, as the session takes it.
        struct Meta {
            config: TurnConfig,
            origin: Option<Origin>,
            label: Label,
            inherited: Option<Inherited>,
            marks: Option<Vec<crate::graph::Mark>>,
            unread_marks: Vec<serde_json::Value>,
        }
        let mut meta: Option<Meta> = None;
        let mut segments: Vec<Segment> = Vec::new();
        let mut turns: Vec<Turn> = Vec::new(); // own-turn contents, in order
        let lines: Vec<String> = BufReader::new(file)
            .lines()
            .collect::<std::io::Result<_>>()?;
        let content: Vec<&str> = lines
            .iter()
            .map(String::as_str)
            .filter(|l| !l.trim().is_empty())
            .collect();
        for (i, line) in content.iter().enumerate() {
            let record = match serde_json::from_str::<Record>(line) {
                Ok(record) => record,
                // A garbled *final* line is almost always a torn write (the
                // process died mid-append): recover the complete prior records
                // rather than declaring the whole episode unreadable. A garbled
                // line anywhere earlier is genuine corruption — fail loudly.
                Err(e) if i + 1 == content.len() => {
                    warn!(
                        episode = %id.to_hex(),
                        error = %e,
                        "ignoring unparseable trailing line (likely an incomplete write)",
                    );
                    break;
                }
                Err(e) => return Err(e.into()),
            };
            match record {
                Record::Episode {
                    id: rid,
                    config,
                    origin,
                    label,
                    inherited,
                    marks,
                    parent,
                } => {
                    if rid != *id {
                        return Err(corrupt(format!(
                            "header id {} does not match the file name",
                            rid.to_hex()
                        )));
                    }
                    let origin = lenient(id, "a branch's origin", origin);
                    let label = lenient(id, "a name", label).unwrap_or_default();
                    // **Absent is not cleared.** Nothing in this build clears
                    // it, and a header without the key is an older build's
                    // re-put — a rename, a `:model` — which erased the record
                    // for good (review stop 2, B 3): the one before stands.
                    // As it does for a record that does not read (review stop
                    // 4, A2 9).
                    let inherited = lenient(id, "an import record", inherited)
                        .or_else(|| meta.as_ref().and_then(|m| m.inherited.clone()));
                    // The same for marks: absent is an older build's header,
                    // and the last ones stand; an empty list clears them.
                    // Each entry read alone: one this build does not know is
                    // kept as it was.
                    let (marks, unread_marks) = match lenient(id, "its marks", marks) {
                        Some(entries) => {
                            let saved = crate::graph::mark::read_saved(entries);
                            if !saved.unread.is_empty() || saved.dropped > 0 {
                                warn!(
                                    episode = %id.to_hex(),
                                    unread = saved.unread.len(),
                                    dropped = saved.dropped,
                                    "saved marks this build does not read: kept as they were",
                                );
                            }
                            (Some(saved.marks), saved.unread)
                        }
                        None => meta
                            .as_ref()
                            .map(|m| (m.marks.clone(), m.unread_marks.clone()))
                            .unwrap_or_default(),
                    };
                    // Migrate a legacy prefix-share parent into an origin + a
                    // leading Slice (only meaningful before any records follow).
                    match parent {
                        // Only migrate the FIRST legacy header (a re-put appends a
                        // fresh header; the shim must add its leading Slice once).
                        Some(legacy) if segments.is_empty() => {
                            segments.push(Segment::Slice {
                                episode: legacy.episode,
                                start: 0,
                                end: legacy.fork_index,
                            });
                            meta = Some(Meta {
                                config,
                                origin: Some(Origin {
                                    episode: legacy.episode,
                                    turn: legacy.turn,
                                    // The legacy fork_index counts SHARED turns;
                                    // the attach turn is the last of them.
                                    index: legacy.fork_index.checked_sub(1),
                                    kind: legacy.kind,
                                }),
                                label,
                                inherited,
                                marks,
                                unread_marks,
                            });
                        }
                        // A new-format header, or a repeated header (whose leading
                        // Slice is already present): update metadata, add no slice.
                        _ => {
                            meta = Some(Meta {
                                config,
                                origin,
                                label,
                                inherited,
                                marks,
                                unread_marks,
                            })
                        }
                    }
                }
                Record::Turn(turn) => {
                    let expected = TurnId::compute(&turn.messages, &turn.provenance.config);
                    if expected != turn.id {
                        return Err(corrupt(format!(
                            "turn {} fails its content hash",
                            turn.id.to_hex()
                        )));
                    }
                    crate::graph::episode::push_own(&mut segments, turn.occurrence());
                    turns.push(turn);
                }
                Record::Slice {
                    episode: src,
                    start,
                    end,
                } => segments.push(Segment::Slice {
                    episode: src,
                    start,
                    end,
                }),
            }
        }

        let Meta {
            config,
            origin,
            label,
            inherited,
            marks,
            unread_marks,
        } = meta.ok_or_else(|| corrupt("no episode header record".into()))?;
        let mut episode = Episode::from_segments(*id, config, segments, origin);
        // As a file says it, which a hand may have edited (`Label::read_back`).
        episode.label = label.read_back();
        episode.inherited = inherited;
        episode.marks = marks;
        episode.unread_marks = unread_marks;
        Ok(LoadedEpisode { episode, turns })
    }

    fn list_episodes(&self) -> StoreResult<Vec<EpisodeId>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(self.root.join("episodes"))? {
            let entry = entry?;
            // Only regular files named `<64-hex>.jsonl` — skip directories (a dir
            // named like an episode would otherwise blow up `load_episode` with
            // an EISDIR `Io` error) and anything else dropped in `episodes/`.
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name();
            if let Some(stem) = name.to_string_lossy().strip_suffix(".jsonl")
                && let Some(id) = EpisodeId::from_hex(stem)
            {
                out.push(id);
            }
        }
        Ok(out)
    }

    fn delete_episode(&mut self, id: &EpisodeId) -> StoreResult<()> {
        // Remove the episode's log file. Idempotent: a missing file is success
        // (already gone), not an error; any other I/O error propagates.
        match fs::remove_file(self.episode_path(id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn put_ref(&mut self, name: &str, episode: &EpisodeId) -> StoreResult<()> {
        validate_ref_name(name)?;
        // Owner-only, overwriting: a ref points at one episode (truncate, not
        // append). Same 0600 posture as the episode logs.
        let mut opts = fs::OpenOptions::new();
        opts.create(true).write(true).truncate(true);
        owner_only(&mut opts);
        let mut file = opts.open(self.ref_path(name))?;
        file.write_all(format!("{}\n", episode.to_hex()).as_bytes())?;
        Ok(())
    }

    fn get_ref(&self, name: &str) -> StoreResult<Option<EpisodeId>> {
        validate_ref_name(name)?;
        match fs::read_to_string(self.ref_path(name)) {
            Ok(body) => {
                let hex = body.trim();
                EpisodeId::from_hex(hex)
                    .map(Some)
                    .ok_or_else(|| StoreError::Corrupt {
                        episode: format!("ref:{name}"),
                        detail: format!("{hex:?} is not a valid episode id"),
                    })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // A line holds a turn whole, so there is nothing less to read: structure is
    // the full graph, and bodies come out of one.
    fn load_structure(&self) -> StoreResult<crate::graph::Graph> {
        self.load_graph()
    }

    fn load_bodies(
        &self,
        ids: &[TurnId],
    ) -> StoreResult<Vec<(TurnId, Vec<crate::message::Message>)>> {
        super::bodies_from_full_load(self, ids)
    }

    fn episode_len(&self, id: &EpisodeId) -> StoreResult<Option<usize>> {
        super::episode_len_by_loading(self, id)
    }
}

#[cfg(test)]
mod tests {
    use super::JsonlStore;
    use crate::graph::{Episode, EpisodeId};
    use crate::store::Store;
    use crate::store::contract::{self, cfg, turn};
    use tempfile::TempDir;

    /// Run a contract assertion against a fresh store in a throwaway temp dir.
    /// The dir lives until the closure returns — and it is *never* the real XDG
    /// state dir.
    fn with_store(test: impl FnOnce(&mut JsonlStore)) {
        let dir = TempDir::new().expect("temp dir");
        let mut store = JsonlStore::new(dir.path()).expect("open store");
        test(&mut store);
    }

    // The shared backend contract (see `store::contract`) — same suite as
    // MemoryStore, now against the on-disk format, each in a throwaway temp
    // dir that lives to the end of its test's one statement.
    contract::contract_tests!(
        JsonlStore::new(TempDir::new().expect("temp dir").path()).expect("open store")
    );

    /// **The double refuses at the depth the disk does** — `MemoryStore`, the
    /// app's store in tests and a fallback in production, measured as this
    /// store measures its lines, not to a limit of its own: it had none, so
    /// no test on it could reach `TooDeep` (review stop 12, T9 7).
    #[test]
    fn the_double_refuses_at_the_depth_the_disk_does() {
        let refuses = |s: &mut dyn Store, depth: usize| {
            let id = EpisodeId::from_seed(format!("deep {depth}").as_bytes());
            s.put_episode(&Episode::root(id, cfg())).unwrap();
            match s.append_turn(&id, &contract::nested_turn(depth)) {
                Ok(()) => false,
                Err(super::StoreError::TooDeep { .. }) => true,
                Err(e) => panic!("at {depth}: {e}"),
            }
        };
        with_store(|disk| {
            let mut double = crate::store::MemoryStore::new();
            let verdicts: Vec<bool> = (110..140).map(|d| refuses(disk, d)).collect();
            assert!(
                verdicts.contains(&true) && verdicts.contains(&false),
                "premise, both sides: {verdicts:?}"
            );
            for (depth, disk_refuses) in (110..140).zip(verdicts) {
                assert_eq!(refuses(&mut double, depth), disk_refuses, "at {depth}");
            }
            // And before asking whether the session exists, as the disk
            // measures its line before it opens the file.
            let unregistered = EpisodeId::from_seed(b"never put");
            for (which, s) in [("disk", disk as &mut dyn Store), ("double", &mut double)] {
                let said = s.append_turn(&unregistered, &contract::nested_turn(139));
                assert!(
                    matches!(said, Err(super::StoreError::TooDeep { .. })),
                    "{which}: {said:?}"
                );
            }
        });
    }
    /// An unnamed session's header is the bytes it always was — no `label`
    /// key — so a log written before names, and one read by an older build,
    /// say the same thing.
    #[test]
    fn an_unnamed_header_writes_no_label() {
        with_store(|store| {
            let id = EpisodeId::from_seed(b"plain");
            store.put_episode(&Episode::root(id, cfg())).unwrap();
            let text = std::fs::read_to_string(store.episode_path(&id)).unwrap();
            assert!(!text.contains("label"), "{text}");
            assert!(!text.contains("inherited"), "nor an import record: {text}");
            assert!(!text.contains("marks"), "nor marks: {text}");
        });
    }
    /// **A legacy `parent` header still loads as a branch** — an origin at
    /// the last shared turn and a leading slice of the shared turns. Off, an
    /// old branch loaded as a root without its parent's turns, and all 1009
    /// core tests passed (review stop 4, A2 5).
    #[test]
    fn a_legacy_parent_header_migrates() {
        use crate::graph::{EdgeKind, Segment};
        with_store(|store| {
            let parent = EpisodeId::from_seed(b"parent");
            let shared = turn("p");
            let id = EpisodeId::from_seed(b"legacy");
            store.put_episode(&Episode::root(id, cfg())).unwrap();
            store.append_turn(&id, &turn("a")).unwrap();
            let path = store.episode_path(&id);
            let text = std::fs::read_to_string(&path).unwrap();
            let (first, rest) = text.split_once('\n').unwrap();
            let mut header: serde_json::Value = serde_json::from_str(first).unwrap();
            header.as_object_mut().unwrap().insert(
                "parent".into(),
                serde_json::json!({
                    "episode": parent,
                    "turn": shared.id,
                    "fork_index": 2,
                    "kind": EdgeKind::Branch,
                }),
            );
            std::fs::write(&path, format!("{header}\n{rest}")).unwrap();
            let loaded = store.load_episode(&id).unwrap().episode;
            let origin = loaded.origin.expect("migrated to an origin");
            assert_eq!((origin.episode, origin.turn), (parent, shared.id));
            assert_eq!(origin.index, Some(1), "the last of the shared turns");
            assert!(
                matches!(
                    loaded.segments().first(),
                    Some(Segment::Slice { episode, start: 0, end: 2 }) if *episode == parent
                ),
                "{:?}",
                loaded.segments()
            );
            // A re-put appends a fresh header — the legacy one again, after
            // the turn — and adds no second slice: only the FIRST header
            // migrates (unheld until review stop 12, T9).
            std::fs::write(&path, format!("{header}\n{rest}{header}\n")).unwrap();
            let loaded = store.load_episode(&id).unwrap().episode;
            let slices = (loaded.segments().iter())
                .filter(|s| matches!(s, Segment::Slice { .. }))
                .count();
            assert_eq!(slices, 1, "{:?}", loaded.segments());
        });
    }

    /// **The loader reads only what is a log of the episode it names** — a
    /// header whose id is not its file's is corrupt, a directory named like a
    /// log is no episode, and a ref naming no episode is corrupt, not unset.
    /// Each guard was deleted green in both crates (review stop 12, T9).
    #[test]
    fn the_loader_refuses_what_is_not_the_log_it_names() {
        use crate::store::StoreError;
        with_store(|store| {
            let (a, b) = (EpisodeId::from_seed(b"a"), EpisodeId::from_seed(b"b"));
            store.put_episode(&Episode::root(a, cfg())).unwrap();
            // A's log under B's name: its header says A.
            std::fs::copy(store.episode_path(&a), store.episode_path(&b)).unwrap();
            assert!(
                matches!(store.load_episode(&b), Err(StoreError::Corrupt { .. })),
                "a header for another episode"
            );
            std::fs::remove_file(store.episode_path(&b)).unwrap();
            // A directory named like a log: `delete all` lists episodes, so
            // one listed here would be a load that fails, or a delete.
            std::fs::create_dir(store.episode_path(&EpisodeId::from_seed(b"c"))).unwrap();
            assert_eq!(store.list_episodes().unwrap(), vec![a]);
            // A ref that names no episode is said, not read as no ref.
            std::fs::write(store.ref_path("HEAD"), "not an id\n").unwrap();
            assert!(matches!(
                store.get_ref("HEAD"),
                Err(StoreError::Corrupt { .. })
            ));
        });
    }
    /// **A header written before import records were reads as born here** —
    /// the line an older build wrote, by hand, with no `inherited` key.
    #[test]
    fn a_header_from_before_import_records_loads() {
        with_store(|store| {
            let id = EpisodeId::from_seed(b"older");
            let mut episode = Episode::root(id, cfg());
            episode.inherited = Some(crate::graph::Inherited {
                session: "cc".into(),
                turns: 1,
                at: None,
                opened: Vec::new(),
            });
            store.put_episode(&episode).unwrap();
            // The old shape by hand: today's header with the field taken
            // out, so this reads a header that LACKS it whatever the writer
            // does — it read what `skip_serializing_if` wrote, and measured
            // the writer (review stop 2, C 10).
            let path = store.episode_path(&id);
            let text = std::fs::read_to_string(&path).unwrap();
            let mut header: serde_json::Value =
                serde_json::from_str(text.lines().next().unwrap()).unwrap();
            let fields = header.as_object_mut().unwrap();
            assert!(
                fields.remove("inherited").is_some(),
                "premise: written, then taken out"
            );
            std::fs::write(&path, format!("{header}\n")).unwrap();
            let loaded = store.load_episode(&id).unwrap().episode;
            assert_eq!(loaded.inherited, None);
        });
    }

    /// A header written with an import record, its turn after it, and the
    /// header's JSON as `edit` leaves it — the shapes a hand or another
    /// build writes, built on disk whatever this build's writer does.
    fn with_header_edited(
        store: &mut JsonlStore,
        edit: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
    ) -> EpisodeId {
        let id = EpisodeId::from_seed(b"imported");
        let mut episode = Episode::root(id, cfg());
        episode.inherited = Some(crate::graph::Inherited {
            session: "cc".into(),
            turns: 1,
            at: None,
            opened: Vec::new(),
        });
        store.put_episode(&episode).unwrap();
        store.append_turn(&id, &turn("a")).unwrap();
        let path = store.episode_path(&id);
        let text = std::fs::read_to_string(&path).unwrap();
        let (first, rest) = text.split_once('\n').unwrap();
        let mut header: serde_json::Value = serde_json::from_str(first).unwrap();
        edit(header.as_object_mut().unwrap());
        std::fs::write(&path, format!("{header}\n{rest}")).unwrap();
        id
    }

    /// **A malformed import record drops alone** (review stop 2, B 2): it
    /// failed the header, and the session vanished from `:graph`, turns and
    /// all, for a presentation field. Now the session loads, its turn with
    /// it, and only the record is gone.
    #[test]
    fn a_malformed_import_record_drops_alone() {
        for bad in [
            serde_json::json!(5),
            serde_json::json!({"session": 1, "turns": 1}),
            serde_json::json!({"session": "cc", "turns": -1}),
            serde_json::json!({"session": "cc"}),
        ] {
            with_store(|store| {
                let id = with_header_edited(store, |h| {
                    h.insert("inherited".into(), bad.clone());
                });
                let loaded = store.load_episode(&id).unwrap();
                assert_eq!(loaded.episode.inherited, None, "{bad}");
                assert_eq!(loaded.turns.len(), 1, "its turn too: {bad}");
                let graph = store.load_graph().unwrap();
                assert!(graph.episode(&id).is_some(), "listed: {bad}");
            });
        }
    }

    /// **A malformed name or origin drops alone too** (docketed at review
    /// stop 2, B 2: read strictly, either failed the header as the import
    /// record did, and the session vanished). The session loads with its
    /// turn and its import record; the name is gone, and a branch whose
    /// origin does not read is a root — in the tree, if out of place.
    #[test]
    fn a_malformed_name_or_origin_drops_alone() {
        for (field, bad) in [
            ("label", serde_json::json!(5)),
            ("label", serde_json::json!({"name": 3})),
            ("origin", serde_json::json!(5)),
            ("origin", serde_json::json!({"episode": "not an id"})),
        ] {
            with_store(|store| {
                let id = with_header_edited(store, |h| {
                    h.insert(field.into(), bad.clone());
                });
                let loaded = store.load_episode(&id).unwrap();
                assert_eq!(loaded.turns.len(), 1, "its turn: {field} {bad}");
                assert!(loaded.episode.label.is_empty(), "{field} {bad}");
                assert_eq!(loaded.episode.origin, None, "{field} {bad}");
                assert!(
                    loaded.episode.inherited.is_some(),
                    "the rest of the header: {field} {bad}"
                );
                let graph = store.load_graph().unwrap();
                assert!(graph.episode(&id).is_some(), "listed: {field} {bad}");
            });
        }
    }

    /// **A header without the record keeps the one before** (review stop 2,
    /// B 3): the last header wins, and an older build's re-put — a rename, a
    /// `:model` — writes none, which erased the record for good. So does a
    /// header whose record does not read (review stop 4, A2 9).
    #[test]
    fn an_import_record_outlives_a_header_without_it() {
        for later in [None, Some(serde_json::json!(5))] {
            with_store(|store| {
                let id = with_header_edited(store, |_| {});
                let path = store.episode_path(&id);
                let text = std::fs::read_to_string(&path).unwrap();
                let mut older: serde_json::Value =
                    serde_json::from_str(text.lines().next().unwrap()).unwrap();
                let header = older.as_object_mut().unwrap();
                header.remove("inherited");
                if let Some(bad) = &later {
                    header.insert("inherited".into(), bad.clone());
                }
                std::fs::write(&path, format!("{text}{older}\n")).unwrap();
                let loaded = store.load_episode(&id).unwrap().episode;
                assert_eq!(
                    loaded.inherited.map(|i| i.session),
                    Some("cc".to_string()),
                    "the earlier header's record stands: {later:?}"
                );
            });
        }
    }

    /// **Marks outlive a header without them** — an older build's re-put
    /// (a rename, a `:model`) writes no `marks` key, and one that does not
    /// read is the same — while an empty list, written, clears them. And a
    /// header carrying them loads in a reader that knows none: an unknown key
    /// is not an error (`Record` denies none).
    #[test]
    fn marks_outlive_a_header_without_them() {
        use crate::graph::{Mark, Whose};
        let mark = Mark {
            name: 'b',
            whose: Whose::Agents,
            turn: 0,
            block: "tool".into(),
            nth: 0,
            line: 0,
            col: 0,
            note: "here".into(),
        };
        for later in [None, Some(serde_json::json!(5))] {
            with_store(|store| {
                let id = EpisodeId::from_seed(b"marked");
                let mut episode = Episode::root(id, cfg());
                episode.marks = Some(vec![mark.clone()]);
                store.put_episode(&episode).unwrap();
                let path = store.episode_path(&id);
                let text = std::fs::read_to_string(&path).unwrap();
                let mut older: serde_json::Value =
                    serde_json::from_str(text.lines().next().unwrap()).unwrap();
                let header = older.as_object_mut().unwrap();
                assert!(header.contains_key("marks"), "premise: written: {header:?}");
                header.remove("marks");
                // And a key this build knows nothing of, as a newer one
                // writes: no error (review stop 14, C 10 — the doc said so
                // and no header held one).
                header.insert("a_newer_builds_key".into(), serde_json::json!({ "x": 1 }));
                if let Some(bad) = &later {
                    header.insert("marks".into(), bad.clone());
                }
                std::fs::write(&path, format!("{text}{older}\n")).unwrap();
                let loaded = store.load_episode(&id).unwrap().episode;
                assert_eq!(
                    loaded.marks,
                    Some(vec![mark.clone()]),
                    "they stand: {later:?}"
                );
                episode.marks = Some(Vec::new());
                store.put_episode(&episode).unwrap();
                let cleared = store.load_episode(&id).unwrap().episode.marks;
                assert_eq!(cleared, Some(Vec::new()), "an empty list clears them");
            });
        }
    }

    /// **A saved mark this build does not read survives its save** (review
    /// stop 14, B F4): a header whose list holds one entry of another shape —
    /// a newer build's `whose` — loads the rest, keeps that one as it was,
    /// and writes it back with the next header, which an older build reading
    /// a newer one's marks erased for good.
    #[test]
    fn a_saved_mark_not_read_is_written_back_as_it_was() {
        use crate::graph::{Mark, Whose};
        let mark = Mark {
            name: 'a',
            whose: Whose::Yours,
            turn: 0,
            block: "you".into(),
            nth: 0,
            line: 0,
            col: 0,
            note: String::new(),
        };
        let newer = serde_json::json!({ "name": "b", "whose": "shared", "turn": 0 });
        with_store(|store| {
            let id = EpisodeId::from_seed(b"newer");
            let mut episode = Episode::root(id, cfg());
            episode.marks = Some(vec![mark.clone()]);
            store.put_episode(&episode).unwrap();
            let path = store.episode_path(&id);
            let text = std::fs::read_to_string(&path).unwrap();
            let mut header: serde_json::Value =
                serde_json::from_str(text.lines().next().unwrap()).unwrap();
            header["marks"].as_array_mut().unwrap().push(newer.clone());
            std::fs::write(&path, format!("{header}\n")).unwrap();
            let mut loaded = store.load_episode(&id).unwrap().episode;
            assert_eq!(loaded.marks, Some(vec![mark.clone()]), "the rest read");
            let kept = std::slice::from_ref(&newer);
            assert_eq!(loaded.unread_marks, kept, "that one kept");
            // A save by this build: a mark of yours set since.
            let mut also = mark.clone();
            also.name = 'z';
            loaded.marks = Some(vec![mark.clone(), also.clone()]);
            store.put_episode(&loaded).unwrap();
            let again = store.load_episode(&id).unwrap().episode;
            assert_eq!(again.marks, Some(vec![mark.clone(), also]));
            assert_eq!(again.unread_marks, kept, "written back as it was");
        });
    }

    /// **Why a field did not read says nothing of what it held** — serde's
    /// message quotes the value, and a header holds a session's name.
    #[test]
    fn an_unread_field_is_described_without_its_value() {
        use super::Lenient;
        let value = serde_json::json!("hunter2seekrit");
        let quoted = serde_json::from_value::<crate::graph::Label>(value.clone())
            .unwrap_err()
            .to_string();
        assert!(
            quoted.contains("hunter2"),
            "premise: serde quotes it: {quoted}"
        );
        let Lenient::Unread(why) =
            serde_json::from_value::<Lenient<crate::graph::Label>>(value).unwrap()
        else {
            panic!("premise: a string is not a label")
        };
        assert!(!why.contains("hunter2") && why.contains("string"), "{why}");
    }

    // --- JSONL-specific guarantees the in-memory backend can't have ---

    #[test]
    fn episode_file_is_line_delimited_inspectable_json() {
        let dir = TempDir::new().unwrap();
        let mut store = JsonlStore::new(dir.path()).unwrap();
        let id = EpisodeId::from_seed(b"e");
        store.put_episode(&Episode::root(id, cfg())).unwrap();
        let t = turn("hello");
        store.append_turn(&id, &t).unwrap();

        let path = dir
            .path()
            .join("episodes")
            .join(format!("{}.jsonl", id.to_hex()));
        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();

        assert_eq!(lines.len(), 2, "header line + one turn line");
        for line in &lines {
            serde_json::from_str::<serde_json::Value>(line).expect("each line is valid JSON");
        }
        assert!(
            lines[0].contains(r#""kind":"episode""#),
            "header: {}",
            lines[0]
        );
        assert!(lines[1].contains(r#""kind":"turn""#), "turn: {}", lines[1]);
        // grep-able by id and by message content.
        assert!(body.contains(&t.id.to_hex()), "turn id is greppable");
        assert!(body.contains("hello"), "message content is greppable");
    }

    #[test]
    fn a_tampered_turn_line_is_rejected_as_corrupt() {
        let dir = TempDir::new().unwrap();
        let mut store = JsonlStore::new(dir.path()).unwrap();
        let id = EpisodeId::from_seed(b"e");
        store.put_episode(&Episode::root(id, cfg())).unwrap();
        store.append_turn(&id, &turn("real")).unwrap();

        // Edit the message content but leave the (now-stale) stored id — exactly
        // what a hand-edit or bit-rot looks like. The content-hash check catches it.
        let path = dir
            .path()
            .join("episodes")
            .join(format!("{}.jsonl", id.to_hex()));
        let tampered = std::fs::read_to_string(&path)
            .unwrap()
            .replace("real", "fake");
        std::fs::write(&path, tampered).unwrap();

        let err = store.load_episode(&id).unwrap_err();
        assert!(
            matches!(err, crate::store::StoreError::Corrupt { .. }),
            "expected Corrupt, got {err:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn persisted_files_and_dirs_are_owner_only() {
        // Conversation history can hold secrets / PII — match the project's
        // 0700-dir / 0600-file posture rather than the process umask.
        use std::os::unix::fs::PermissionsExt as _;
        let dir = TempDir::new().unwrap();
        let mut store = JsonlStore::new(dir.path()).unwrap();
        let id = EpisodeId::from_seed(b"e");
        store.put_episode(&Episode::root(id, cfg())).unwrap();
        let secret = turn("secret");
        store.append_turn(&id, &secret).unwrap();
        store.put_ref("HEAD", &id).unwrap();
        // A rewrite replaces the log with a new file, which must be born
        // owner-only too: its mode was held by nothing (review stop 12, T9 2b).
        let mut rewritten = Episode::root(id, cfg());
        rewritten.append(secret.occurrence());
        store.rewrite_episode(&rewritten, &[secret]).unwrap();

        let mode =
            |p: std::path::PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let ep = dir
            .path()
            .join("episodes")
            .join(format!("{}.jsonl", id.to_hex()));
        assert_eq!(mode(ep), 0o600, "episode log is owner-only");
        assert_eq!(
            mode(dir.path().join("episodes")),
            0o700,
            "episodes dir owner-only"
        );
        assert_eq!(
            mode(dir.path().join("refs").join("HEAD")),
            0o600,
            "ref owner-only"
        );
    }

    /// **A refused rewrite leaves nothing beside the log** — its half-written
    /// replacement is removed, so the next rewrite starts clean and no second
    /// copy of the conversation sits on disk (review stop 12, T9 2a; the
    /// log itself is the contract's `a_rewrite_that_fails_leaves_the_log`).
    #[test]
    fn a_refused_rewrite_leaves_no_replacement_behind() {
        with_store(|store| {
            let id = EpisodeId::from_seed(b"e");
            store.put_episode(&Episode::root(id, cfg())).unwrap();
            let (a, deep) = (turn("a"), contract::nested_turn(140));
            store.append_turn(&id, &a).unwrap();
            let mut episode = Episode::root(id, cfg());
            episode.append(a.occurrence());
            episode.append(deep.occurrence());
            assert!(store.rewrite_episode(&episode, &[a, deep]).is_err());
            let log = store.episode_path(&id);
            let dir = log.parent().expect("the episodes dir");
            let names: Vec<_> = (std::fs::read_dir(dir).unwrap())
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert_eq!(names, [log.file_name().unwrap()], "{names:?}");
        });
    }

    #[test]
    fn recovers_prior_turns_from_a_torn_trailing_line() {
        // Simulate a crash mid-append: the last line is truncated, unparseable
        // JSON. The complete prior turns must still load.
        let dir = TempDir::new().unwrap();
        let id = EpisodeId::from_seed(b"e");
        let path = {
            let mut store = JsonlStore::new(dir.path()).unwrap();
            store.put_episode(&Episode::root(id, cfg())).unwrap();
            store.append_turn(&id, &turn("a")).unwrap();
            store.append_turn(&id, &turn("b")).unwrap();
            dir.path()
                .join("episodes")
                .join(format!("{}.jsonl", id.to_hex()))
        };
        let mut body = std::fs::read_to_string(&path).unwrap();
        body.push_str(r#"{"kind":"turn","id":"deadbe"#); // torn: no close, no newline
        std::fs::write(&path, body).unwrap();

        let loaded = JsonlStore::new(dir.path())
            .unwrap()
            .load_episode(&id)
            .unwrap();
        assert_eq!(
            loaded.turns.len(),
            2,
            "the two complete turns are recovered"
        );
    }

    /// **A torn line is not where the next turn goes** (review stop 12, T9
    /// 1): appended onto the fragment, the next turn fused with it and was
    /// dropped as a torn line, and the one after left the fused line in the
    /// middle — unreadable, so the whole session left `:graph`. Appended
    /// over, every complete turn loads and the session is listed.
    #[test]
    fn appending_after_a_torn_line_keeps_every_turn() {
        let dir = TempDir::new().unwrap();
        let id = EpisodeId::from_seed(b"e");
        let mut store = JsonlStore::new(dir.path()).unwrap();
        store.put_episode(&Episode::root(id, cfg())).unwrap();
        store.append_turn(&id, &turn("a")).unwrap();
        let path = store.episode_path(&id);
        let mut body = std::fs::read_to_string(&path).unwrap();
        body.push_str(r#"{"kind":"turn","id":"deadbe"#); // torn: no close, no newline
        std::fs::write(&path, body).unwrap();

        store.append_turn(&id, &turn("b")).unwrap();
        store.append_turn(&id, &turn("c")).unwrap();
        let reopened = JsonlStore::new(dir.path()).unwrap();
        let loaded = reopened.load_episode(&id).unwrap();
        assert_eq!(loaded.turns.len(), 3, "a, b and c");
        let graph = reopened.load_graph().unwrap();
        assert!(graph.episode(&id).is_some(), "and the session is listed");
    }

    /// **A whole last record without its newline is kept** — completed, not
    /// cut: only a tail that does not parse is a torn write.
    #[test]
    fn appending_after_an_unterminated_record_keeps_it() {
        let dir = TempDir::new().unwrap();
        let id = EpisodeId::from_seed(b"e");
        let mut store = JsonlStore::new(dir.path()).unwrap();
        store.put_episode(&Episode::root(id, cfg())).unwrap();
        store.append_turn(&id, &turn("a")).unwrap();
        store.append_turn(&id, &turn("b")).unwrap();
        let path = store.episode_path(&id);
        let body = std::fs::read_to_string(&path).unwrap();
        let unterminated = body.strip_suffix('\n').expect("premise: a line ends it");
        std::fs::write(&path, unterminated).unwrap();

        store.append_turn(&id, &turn("c")).unwrap();
        let loaded = JsonlStore::new(dir.path())
            .unwrap()
            .load_episode(&id)
            .unwrap();
        assert_eq!(loaded.turns.len(), 3, "a, b (kept) and c");
    }

    #[test]
    fn a_corrupt_non_trailing_line_still_errors() {
        // Garbage that isn't the last line is genuine corruption, not a torn
        // write — it must fail loudly rather than silently drop a middle turn.
        let dir = TempDir::new().unwrap();
        let id = EpisodeId::from_seed(b"e");
        let path = {
            let mut store = JsonlStore::new(dir.path()).unwrap();
            store.put_episode(&Episode::root(id, cfg())).unwrap();
            store.append_turn(&id, &turn("a")).unwrap();
            dir.path()
                .join("episodes")
                .join(format!("{}.jsonl", id.to_hex()))
        };
        // Rewrite as [header, GARBAGE, turn] so the garbage line isn't last.
        let existing = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<&str> = existing.lines().collect();
        let turn_a = lines.pop().unwrap();
        let header = lines.pop().unwrap();
        std::fs::write(&path, format!("{header}\nnot valid json\n{turn_a}\n")).unwrap();

        let err = JsonlStore::new(dir.path())
            .unwrap()
            .load_episode(&id)
            .unwrap_err();
        assert!(
            matches!(err, crate::store::StoreError::Serde(_)),
            "non-trailing garbage errors, got {err:?}"
        );
    }

    #[test]
    fn load_graph_skips_an_unloadable_episode() {
        // One corrupt episode must not deny the user every other episode.
        let dir = TempDir::new().unwrap();
        let mut store = JsonlStore::new(dir.path()).unwrap();
        let good = EpisodeId::from_seed(b"good");
        let bad = EpisodeId::from_seed(b"bad");
        store.put_episode(&Episode::root(good, cfg())).unwrap();
        store.append_turn(&good, &turn("g")).unwrap();
        store.put_episode(&Episode::root(bad, cfg())).unwrap();

        let bad_path = dir
            .path()
            .join("episodes")
            .join(format!("{}.jsonl", bad.to_hex()));
        std::fs::write(&bad_path, "not json\nstill not json\n").unwrap();

        let graph = store.load_graph().unwrap();
        assert!(graph.episode(&good).is_some(), "good episode loaded");
        assert!(graph.episode(&bad).is_none(), "corrupt episode skipped");
    }

    #[test]
    fn data_survives_reopening_the_store() {
        // The real point of persistence: a brand-new store over the same dir
        // sees what the previous one wrote.
        let dir = TempDir::new().unwrap();
        let id = EpisodeId::from_seed(b"persist");
        {
            let mut store = JsonlStore::new(dir.path()).unwrap();
            store.put_episode(&Episode::root(id, cfg())).unwrap();
            store.append_turn(&id, &turn("a")).unwrap();
            store.append_turn(&id, &turn("b")).unwrap();
            store.put_ref("HEAD", &id).unwrap();
        }
        let store = JsonlStore::new(dir.path()).unwrap();
        let loaded = store.load_episode(&id).unwrap();
        assert_eq!(loaded.turns.len(), 2);
        assert_eq!(store.get_ref("HEAD").unwrap(), Some(id));
        assert_eq!(store.list_episodes().unwrap(), vec![id]);
    }
}
