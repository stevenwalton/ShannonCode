//! [`LazyBodies`] — any [`Store`], made to load its structure without bodies.
//!
//! **Why this exists.** A backend that can load less (SQLite: bodies in a
//! table of their own) hands the app a graph whose turns carry outlines and no
//! words, and every path that reads words must [`hydrate`](super::hydrate)
//! first. JSONL cannot load less, so it loads everything — and the app's
//! tests, most of which run on JSONL so they can check what reached disk,
//! never met an unloaded body. A reader that forgot to hydrate passed all of
//! them. Wrapped in this, a store answers the way the lazy backends will, so
//! the suite fails where production would (`[[feedback_wiring_is_untested]]`).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{LoadedEpisode, Store, StoreResult};
use crate::graph::{Episode, EpisodeId, Graph, Turn, TurnId};
use crate::message::Message;

/// `inner`, with [`load_structure`](Store::load_structure) holding outlines
/// only. Every other call passes straight through — the bodies are still there
/// for [`load_bodies`](Store::load_bodies) to read.
pub struct LazyBodies {
    inner: Box<dyn Store>,
    /// How many times anything asked for bodies. On a full-loading store each
    /// ask re-reads the whole history, so "asks nothing when nothing is
    /// missing" is a cost the suite can only hold by counting.
    body_loads: Arc<AtomicUsize>,
}

impl LazyBodies {
    pub fn new(inner: Box<dyn Store>) -> Self {
        Self::counted(inner).0
    }

    /// [`new`](Self::new), and a handle on the count of `load_bodies` calls —
    /// returned rather than read off the store because the store is boxed
    /// into whatever it is given to.
    pub fn counted(inner: Box<dyn Store>) -> (Self, Arc<AtomicUsize>) {
        let body_loads = Arc::new(AtomicUsize::new(0));
        let store = Self {
            inner,
            body_loads: Arc::clone(&body_loads),
        };
        (store, body_loads)
    }
}

impl Store for LazyBodies {
    fn put_episode(&mut self, episode: &Episode) -> StoreResult<()> {
        self.inner.put_episode(episode)
    }

    fn append_turn(&mut self, episode: &EpisodeId, turn: &Turn) -> StoreResult<()> {
        self.inner.append_turn(episode, turn)
    }

    fn append_slice(
        &mut self,
        episode: &EpisodeId,
        source: &EpisodeId,
        start: usize,
        end: usize,
    ) -> StoreResult<()> {
        self.inner.append_slice(episode, source, start, end)
    }

    fn rewrite_episode(&mut self, episode: &Episode, own: &[Turn]) -> StoreResult<()> {
        self.inner.rewrite_episode(episode, own)
    }

    fn load_episode(&self, id: &EpisodeId) -> StoreResult<LoadedEpisode> {
        self.inner.load_episode(id)
    }

    fn list_episodes(&self) -> StoreResult<Vec<EpisodeId>> {
        self.inner.list_episodes()
    }

    fn delete_episode(&mut self, id: &EpisodeId) -> StoreResult<()> {
        self.inner.delete_episode(id)
    }

    fn put_ref(&mut self, name: &str, episode: &EpisodeId) -> StoreResult<()> {
        self.inner.put_ref(name, episode)
    }

    fn get_ref(&self, name: &str) -> StoreResult<Option<EpisodeId>> {
        self.inner.get_ref(name)
    }

    fn load_graph(&self) -> StoreResult<Graph> {
        self.inner.load_graph()
    }

    fn load_structure(&self) -> StoreResult<Graph> {
        let mut graph = self.inner.load_structure()?;
        graph.evict_bodies();
        Ok(graph)
    }

    fn load_bodies(&self, ids: &[TurnId]) -> StoreResult<Vec<(TurnId, Vec<Message>)>> {
        self.body_loads.fetch_add(1, Ordering::SeqCst);
        self.inner.load_bodies(ids)
    }

    fn episode_len(&self, id: &EpisodeId) -> StoreResult<Option<usize>> {
        self.inner.episode_len(id)
    }
}

#[cfg(test)]
mod tests {
    use super::LazyBodies;
    use crate::store::{MemoryStore, Store, contract, hydrate};

    // The whole contract still holds through the wrapper: it changes what a
    // structure load carries, never what a store keeps.
    contract::contract_tests!(LazyBodies::new(Box::new(MemoryStore::new())));

    /// …and over a store whose own structure load is a full one — the case
    /// the wrapper exists for.
    #[test]
    fn the_lazy_pair_matches_a_full_load_over_a_full_loading_store() {
        contract::the_lazy_pair_matches_a_full_load(&mut LazyBodies::new(Box::new(
            crate::store::JsonlStore::new(tempfile::TempDir::new().unwrap().path()).unwrap(),
        )));
    }

    /// The subject: a structure load holds outlines and no words, and
    /// [`hydrate`] brings the words back — over a store (JSONL) whose own
    /// structure load is a full load.
    #[test]
    fn a_full_loading_store_answers_structure_without_bodies() {
        use crate::graph::{Episode, EpisodeId, Turn};
        use crate::message::Message;

        let dir = tempfile::TempDir::new().unwrap();
        let mut s = LazyBodies::new(Box::new(crate::store::JsonlStore::new(dir.path()).unwrap()));
        let id = EpisodeId::from_seed(b"lazy");
        let turn = Turn::seal(
            vec![
                Message::user("\n  first words\nmore"),
                Message::assistant("reply"),
            ],
            contract::cfg(),
            std::time::SystemTime::UNIX_EPOCH,
        );
        s.put_episode(&Episode::root(id, contract::cfg())).unwrap();
        s.append_turn(&id, &turn).unwrap();

        let mut graph = s.load_structure().unwrap();
        assert_eq!(
            graph.full_turns(&id),
            vec![turn.id],
            "the structure is all there"
        );
        assert_eq!(graph.messages(&turn.id), None, "and no words");
        assert_eq!(
            graph.records(&id),
            None,
            "so a record is refused, not shortened"
        );
        let outline = graph.outline(&turn.id).expect("an outline");
        assert_eq!(
            outline[0].first_line, "first words",
            "the first line with text on it"
        );
        assert_eq!(outline[0].chars, "\n  first words\nmore".chars().count());

        hydrate(&s, &mut graph, &[id]).unwrap();
        assert_eq!(
            graph.records(&id),
            Some(vec![turn]),
            "hydrated: the words are back"
        );
    }

    /// **A graph that holds the words asks the store for nothing.** On a
    /// full-loading store every body request re-reads the whole history, so a
    /// door that asked anyway doubled the cost of every reading verb — and
    /// both guards against it (the door's and the default `load_bodies`') had
    /// nothing holding them (Reviewers B and C).
    #[test]
    fn a_hydrated_graph_asks_the_store_nothing() {
        use crate::graph::{Episode, EpisodeId};
        use std::sync::atomic::Ordering;

        let (mut s, loads) = LazyBodies::counted(Box::new(MemoryStore::new()));
        let id = EpisodeId::from_seed(b"warm");
        s.put_episode(&Episode::root(id, contract::cfg())).unwrap();
        s.append_turn(&id, &contract::turn("x")).unwrap();

        let mut graph = s.load_structure().unwrap();
        hydrate(&s, &mut graph, &[id]).unwrap();
        assert_eq!(
            loads.load(Ordering::SeqCst),
            1,
            "premise: one load for the cold graph"
        );
        hydrate(&s, &mut graph, &[id]).unwrap();
        assert_eq!(
            loads.load(Ordering::SeqCst),
            1,
            "warm: the store is not asked"
        );
    }

    /// A full-loading store's `load_bodies` — what JSONL ships,
    /// [`bodies_from_full_load`](crate::store::bodies_from_full_load) — reads
    /// the whole graph, so asking it for nothing must not read anything.
    /// Counted at `load_graph`, the cost itself: an empty answer is the same
    /// either way.
    #[test]
    fn the_default_body_load_reads_nothing_for_nothing() {
        use crate::graph::{Episode, Graph, Turn, TurnId};
        use crate::message::Message;
        use crate::store::{LoadedEpisode, StoreResult, bodies_from_full_load};
        use std::cell::Cell;

        /// A full-loading store, counting its full reads.
        #[derive(Default)]
        struct Full {
            inner: MemoryStore,
            reads: Cell<usize>,
        }
        impl Store for Full {
            fn put_episode(&mut self, e: &Episode) -> StoreResult<()> {
                self.inner.put_episode(e)
            }
            fn append_turn(&mut self, e: &EpisodeId, t: &Turn) -> StoreResult<()> {
                self.inner.append_turn(e, t)
            }
            fn append_slice(
                &mut self,
                e: &EpisodeId,
                s: &EpisodeId,
                a: usize,
                b: usize,
            ) -> StoreResult<()> {
                self.inner.append_slice(e, s, a, b)
            }
            fn rewrite_episode(&mut self, e: &Episode, own: &[Turn]) -> StoreResult<()> {
                self.inner.rewrite_episode(e, own)
            }
            fn load_episode(&self, id: &EpisodeId) -> StoreResult<LoadedEpisode> {
                self.inner.load_episode(id)
            }
            fn list_episodes(&self) -> StoreResult<Vec<EpisodeId>> {
                self.inner.list_episodes()
            }
            fn delete_episode(&mut self, id: &EpisodeId) -> StoreResult<()> {
                self.inner.delete_episode(id)
            }
            fn put_ref(&mut self, n: &str, e: &EpisodeId) -> StoreResult<()> {
                self.inner.put_ref(n, e)
            }
            fn get_ref(&self, n: &str) -> StoreResult<Option<EpisodeId>> {
                self.inner.get_ref(n)
            }
            fn load_graph(&self) -> StoreResult<Graph> {
                self.reads.set(self.reads.get() + 1);
                self.inner.load_graph()
            }
            fn load_structure(&self) -> StoreResult<Graph> {
                self.load_graph()
            }
            fn load_bodies(&self, ids: &[TurnId]) -> StoreResult<Vec<(TurnId, Vec<Message>)>> {
                bodies_from_full_load(self, ids)
            }
            fn episode_len(&self, id: &EpisodeId) -> StoreResult<Option<usize>> {
                self.inner.episode_len(id)
            }
        }
        use crate::graph::EpisodeId;

        let s = Full::default();
        assert!(s.load_bodies(&[]).unwrap().is_empty());
        assert_eq!(s.reads.get(), 0, "asking for nothing reads nothing");
    }

    /// An outline keeps a bounded first line, however long the message — the
    /// structure a launch loads must not carry a multi-megabyte tool output.
    #[test]
    fn an_outline_line_is_bounded() {
        use crate::graph::{OUTLINE_CHARS, outline_of};
        use crate::message::Message;
        let long = "é".repeat(1 << 20);
        let outline = outline_of(&[Message::user(long.clone())]);
        assert_eq!(outline[0].first_line.chars().count(), OUTLINE_CHARS);
        assert_eq!(
            outline[0].chars,
            long.chars().count(),
            "the length is the whole"
        );
    }
}
