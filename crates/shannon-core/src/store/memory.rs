//! [`MemoryStore`] — an in-memory [`Store`] backend.
//!
//! The reference implementation: it makes the [`Store`] contract concrete with
//! no I/O, so the contract suite (and any consumer's tests) can run fast and
//! deterministically, and so "what a `Store` means" is pinned by code that
//! can't blame the filesystem. It is also the natural default for a frontend
//! that doesn't want persistence (ephemeral sessions) — design.md §1.3.1's
//! "embedded users who want in-memory."
//!
//! It mirrors the content-addressed split exactly (see the [`super`] module
//! docs): episode *metadata* and the per-episode segments — each own turn's
//! occurrence, provenance included — live in one map; turn *contents* live in a
//! second, keyed by their content-address (so a turn shared across episodes,
//! or produced twice, is stored once).

use std::collections::HashMap;

use crate::graph::{
    Episode, EpisodeId, Graph, Inherited, Label, Origin, Segment, Turn, TurnConfig, TurnId,
};
use crate::message::Message;

use super::{LoadedEpisode, Store, StoreError, StoreResult, validate_ref_name};

/// An episode's persisted state: its metadata plus the [`Segment`]s it accrued, in
/// append order (own turns extend the trailing `Own`; slices are pushed by
/// [`append_slice`](Store::append_slice)). Turn *contents* live in
/// `MemoryStore::contents`, not here — a `Slice` is a reference, never a copy.
#[derive(Debug, Clone)]
struct Entry {
    config: TurnConfig,
    origin: Option<Origin>,
    label: Label,
    inherited: Option<Inherited>,
    marks: Option<Vec<crate::graph::Mark>>,
    unread_marks: Vec<serde_json::Value>,
    segments: Vec<Segment>,
}

impl Entry {
    /// `episode`'s metadata over `segments` — what both writers keep, so a
    /// field the header gains is kept by both.
    fn of(episode: &Episode, segments: Vec<Segment>) -> Entry {
        Entry {
            config: episode.config.clone(),
            origin: episode.origin,
            label: episode.label.clone(),
            inherited: episode.inherited.clone(),
            marks: episode.marks.clone(),
            unread_marks: episode.unread_marks.clone(),
            segments,
        }
    }

    /// The episode this entry holds — what both loads rebuild. Its label as
    /// the real store reads one back (`Label::read_back`): a name no person
    /// could give loads as none here too, not as it was put (review stop 12,
    /// T9 7).
    fn episode(&self, id: EpisodeId) -> Episode {
        let mut episode =
            Episode::from_segments(id, self.config.clone(), self.segments.clone(), self.origin);
        episode.label = self.label.clone().read_back();
        episode.inherited = self.inherited.clone();
        episode.marks = self.marks.clone();
        episode.unread_marks = self.unread_marks.clone();
        episode
    }
}

/// `turn`, if the real store could read it back — measured as its line is
/// ([`check_readable`](super::check_readable)).
fn readable(turn: &Turn) -> StoreResult<()> {
    super::check_readable(&serde_json::to_string(turn)?)
}

/// An in-memory [`Store`]. Cheap to create; holds everything in maps.
#[derive(Debug, Default)]
pub struct MemoryStore {
    /// Episode metadata + append order, by episode id.
    episodes: HashMap<EpisodeId, Entry>,
    /// Turn contents by content-address (shared across episodes). Messages
    /// only: a record keyed here would let one production's facts overwrite
    /// another's.
    contents: HashMap<TurnId, Vec<Message>>,
    /// Named refs → episode id.
    refs: HashMap<String, EpisodeId>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Store for MemoryStore {
    fn put_episode(&mut self, episode: &Episode) -> StoreResult<()> {
        // Metadata only; segments are established by append_turn / append_slice.
        // Re-put updates metadata but keeps any segments already appended.
        let (segments, marks, unread) = self
            .episodes
            .remove(&episode.id)
            .map(|e| (e.segments, e.marks, e.unread_marks))
            .unwrap_or_default();
        let mut entry = Entry::of(episode, segments);
        // A header without marks keeps the last, as the log's reader does
        // (`jsonl.rs`), and what it did not read with them; an empty list
        // clears them.
        if entry.marks.is_none() {
            entry.marks = marks;
            entry.unread_marks = unread;
        }
        self.episodes.insert(episode.id, entry);
        Ok(())
    }

    fn rewrite_episode(&mut self, episode: &Episode, own: &[Turn]) -> StoreResult<()> {
        // In memory the segment list is just a field, so the "atomic" part is
        // free: the map entry is replaced in one assignment or not at all. The
        // completeness check still runs, because a store double that accepts a
        // log the real one rejects makes every test built on it a lie about
        // production ([[feedback_wiring_is_untested]]).
        super::check_own_present(episode, own)?;
        // Every turn the real store would write, measured before anything
        // changes: a refused rewrite leaves the episode as it was.
        for occurrence in episode.own_occurrences() {
            if let Some(content) = own.iter().find(|t| t.id == occurrence.turn) {
                readable(content)?;
            }
        }
        for turn in own {
            self.contents
                .entry(turn.id)
                .or_insert_with(|| turn.messages.clone());
        }
        self.episodes
            .insert(episode.id, Entry::of(episode, episode.segments().to_vec()));
        Ok(())
    }

    fn append_turn(&mut self, episode: &EpisodeId, turn: &Turn) -> StoreResult<()> {
        // Measured first, as the real store measures its line before it opens
        // the file.
        readable(turn)?;
        let entry = self
            .episodes
            .get_mut(episode)
            .ok_or_else(|| StoreError::EpisodeNotFound(episode.to_hex()))?;
        crate::graph::episode::push_own(&mut entry.segments, turn.occurrence());
        self.contents
            .entry(turn.id)
            .or_insert_with(|| turn.messages.clone());
        Ok(())
    }

    fn append_slice(
        &mut self,
        episode: &EpisodeId,
        source: &EpisodeId,
        start: usize,
        end: usize,
    ) -> StoreResult<()> {
        let entry = self
            .episodes
            .get_mut(episode)
            .ok_or_else(|| StoreError::EpisodeNotFound(episode.to_hex()))?;
        entry.segments.push(Segment::Slice {
            episode: *source,
            start,
            end,
        });
        Ok(())
    }

    fn load_episode(&self, id: &EpisodeId) -> StoreResult<LoadedEpisode> {
        let entry = self
            .episodes
            .get(id)
            .ok_or_else(|| StoreError::EpisodeNotFound(id.to_hex()))?;

        let episode = entry.episode(*id);
        // The episode's own-turn records, in order: each occurrence's facts with
        // the shared content (a Slice is derived, so its turns are loaded via
        // the episode it references, not re-stored here).
        let mut turns = Vec::new();
        for occurrence in episode.own_occurrences() {
            let messages =
                self.contents
                    .get(&occurrence.turn)
                    .ok_or_else(|| StoreError::Corrupt {
                        episode: id.to_hex(),
                        detail: format!(
                            "turn {} referenced but not stored",
                            occurrence.turn.to_hex()
                        ),
                    })?;
            turns.push(Turn::of(occurrence, messages.clone()));
        }
        Ok(LoadedEpisode { episode, turns })
    }

    fn list_episodes(&self) -> StoreResult<Vec<EpisodeId>> {
        Ok(self.episodes.keys().copied().collect())
    }

    /// Structure and outlines, **no bodies** — the shape a store with bodies in
    /// a table of their own gives, so every test on this reference store meets
    /// unloaded bodies the way production will, and a reader that forgot to
    /// [`hydrate`](super::hydrate) fails here first.
    fn load_structure(&self) -> StoreResult<Graph> {
        let mut graph = Graph::new();
        for (id, entry) in &self.episodes {
            let episode = entry.episode(*id);
            for occurrence in episode.own_occurrences() {
                if let Some(messages) = self.contents.get(&occurrence.turn) {
                    graph.insert_outline(
                        occurrence.turn,
                        occurrence.provenance.config.model.clone(),
                        crate::graph::outline_of(messages),
                    );
                }
            }
            graph.insert_episode(episode);
        }
        Ok(graph)
    }

    fn load_bodies(&self, ids: &[TurnId]) -> StoreResult<Vec<(TurnId, Vec<Message>)>> {
        Ok(ids
            .iter()
            .filter_map(|id| Some((*id, self.contents.get(id)?.clone())))
            .collect())
    }

    fn episode_len(&self, id: &EpisodeId) -> StoreResult<Option<usize>> {
        Ok(self.episodes.get(id).map(|e| {
            e.segments
                .iter()
                .map(|s| match s {
                    Segment::Own(own) => own.len(),
                    Segment::Slice { .. } => 0,
                })
                .sum()
        }))
    }

    fn delete_episode(&mut self, id: &EpisodeId) -> StoreResult<()> {
        // Drop the episode's metadata + append order. Turn *contents* are
        // content-addressed and may be shared, so they're left in `turns`
        // (harmless — unreferenced and never re-listed). Idempotent.
        self.episodes.remove(id);
        Ok(())
    }

    fn put_ref(&mut self, name: &str, episode: &EpisodeId) -> StoreResult<()> {
        validate_ref_name(name)?;
        self.refs.insert(name.to_string(), *episode);
        Ok(())
    }

    fn get_ref(&self, name: &str) -> StoreResult<Option<EpisodeId>> {
        validate_ref_name(name)?;
        Ok(self.refs.get(name).copied())
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryStore;

    // The shared backend contract (see `store::contract`), one test per
    // function so a failure names the backend + the behavior.
    crate::store::contract::contract_tests!(MemoryStore::new());
}
