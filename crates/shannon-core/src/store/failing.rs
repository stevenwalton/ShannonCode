//! A [`Store`] that can be told to fail — the test double for "what does the
//! app say when the disk says no?"
//!
//! **Why this exists.** Two guards in the TUI carried comments admitting they
//! were untested, for the same reason: there was no way to *make* a write fail.
//! `stop_work`'s `lost` accounting (the "stopped N sessions, but M could NOT be
//! saved to disk" line) and `persist_job_turns`' registration branch both
//! depend on an append refusing, and every store in the tree succeeds. So the
//! honesty guarantee they implement — a partial that was billed but did not
//! reach disk must never be reported as safe — was believed rather than
//! checked, and one of those comments had already claimed a verification that
//! only ever existed in a reviewer's scratch worktree.
//!
//! A test double is the fix for that class: when a stub cannot *distinguish*
//! the guarded behaviour from the unguarded one, no assertion over it can
//! either (`[[feedback_mutation_check_guards]]`).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::{LoadedEpisode, MemoryStore, Store, StoreError, StoreResult};
use crate::graph::{Episode, EpisodeId, Turn};

/// Wraps a real (in-memory) store and fails writes on demand.
///
/// Reads always succeed and always reflect what actually landed, so a test can
/// assert on the *difference* between what the app believes and what is on
/// disk — which is the whole subject.
pub struct FailingStore {
    inner: MemoryStore,
    /// Appends still to be allowed through before the first refusal. `0` fails
    /// every append; `usize::MAX` (the default) never fails.
    appends_ok: AtomicUsize,
    /// Whether `put_episode` refuses — a switch, so a session can land and
    /// its next header fail ([`refusing_headers`](Self::refusing_headers)).
    reject_put: Arc<AtomicBool>,
    /// A refusal that can be **switched off mid-test**, for the repair case.
    ///
    /// The budget above can only count downward, so it expresses "fails from
    /// now on" and never "failed, and then the disk came back". `:write` exists
    /// precisely for that window, and a double that fails forever cannot
    /// express the state the feature is for — it would assert the retry is
    /// *refused*, which is the opposite of the guarantee
    /// (`[[feedback_fixture_omits_the_axis]]`).
    switch: Arc<AtomicBool>,
    /// Episodes whose `delete_episode` refuses — filled in mid-test, once
    /// their ids exist, for a batch delete that goes part-way.
    refused_deletes: Arc<Mutex<Vec<EpisodeId>>>,
    /// How many times the whole structure was read — what a batch's cost is
    /// counted in ([`structure_loads`](Self::structure_loads)).
    structure_loads: Arc<AtomicUsize>,
}

impl FailingStore {
    /// Fail **every** append. Reads and metadata still work.
    pub fn failing_appends() -> Self {
        Self {
            inner: MemoryStore::new(),
            appends_ok: AtomicUsize::new(0),
            reject_put: Arc::default(),
            switch: Arc::new(AtomicBool::new(false)),
            refused_deletes: Arc::default(),
            structure_loads: Arc::default(),
        }
    }

    /// Allow `n` appends, then refuse the rest — for the partial-write shape,
    /// where some of a session's turns land and some do not.
    pub fn failing_after(n: usize) -> Self {
        Self {
            inner: MemoryStore::new(),
            appends_ok: AtomicUsize::new(n),
            reject_put: Arc::default(),
            switch: Arc::new(AtomicBool::new(false)),
            refused_deletes: Arc::default(),
            structure_loads: Arc::default(),
        }
    }

    /// Also refuse `put_episode`, so an episode can never be registered.
    pub fn rejecting_registration(self) -> Self {
        self.reject_put.store(true, Ordering::SeqCst);
        self
    }

    /// A store that writes everything until the returned switch is set, and
    /// then refuses every header (`put_episode`) — a session that landed,
    /// whose next preference or mark cannot be saved. Returned, as
    /// [`recoverable`](Self::recoverable)'s switch is, for a store `Box`ed
    /// out of the test's reach.
    pub fn refusing_headers() -> (Self, Arc<AtomicBool>) {
        let store = Self::failing_after(usize::MAX);
        let switch = Arc::clone(&store.reject_put);
        (store, switch)
    }

    /// A store that fails until the returned switch is set to `false`.
    ///
    /// Returns the handle rather than a method on the store because the store
    /// is `Box`ed into the app and no longer reachable by the test. The window
    /// between "it failed" and "the disk is back" is the whole subject.
    pub fn recoverable() -> (Self, Arc<AtomicBool>) {
        let switch = Arc::new(AtomicBool::new(true));
        (
            Self {
                inner: MemoryStore::new(),
                appends_ok: AtomicUsize::new(usize::MAX),
                reject_put: Arc::default(),
                switch: Arc::clone(&switch),
                refused_deletes: Arc::default(),
                structure_loads: Arc::default(),
            },
            switch,
        )
    }

    /// A store that writes everything and refuses to **delete** the episodes
    /// in the returned list — empty at first, filled in once their ids
    /// exist. Returned, as [`recoverable`](Self::recoverable)'s switch is,
    /// because the store is `Box`ed into the app and out of the test's reach.
    pub fn refusing_deletes() -> (Self, Arc<Mutex<Vec<EpisodeId>>>) {
        let refused: Arc<Mutex<Vec<EpisodeId>>> = Arc::default();
        (
            Self {
                inner: MemoryStore::new(),
                appends_ok: AtomicUsize::new(usize::MAX),
                reject_put: Arc::default(),
                switch: Arc::new(AtomicBool::new(false)),
                refused_deletes: Arc::clone(&refused),
                structure_loads: Arc::default(),
            },
            refused,
        )
    }

    /// How many times the whole structure has been read: returned, as the
    /// switch is, for a test whose store is `Box`ed out of its reach — a
    /// batch reading it once per member cost 3.7 s at 160 sessions (review
    /// stop 4, A2 3).
    pub fn structure_loads(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.structure_loads)
    }

    /// Take one from the budget; `true` when this append may proceed.
    ///
    /// `fetch_update`, not `try_update`: one call under two names, the new
    /// one stable only from 1.95 (past the workspace's floor) and the old
    /// one deprecated from 1.99 — hence the `allow`, which a toolchain
    /// before the rename never needs.
    #[allow(deprecated)]
    fn allow_append(&self) -> bool {
        if self.switch.load(Ordering::SeqCst) {
            return false;
        }
        self.appends_ok
            // `then`, not `then_some`: `then_some` evaluates its argument
            // unconditionally, so `n - 1` underflows the moment the budget hits
            // zero — which is the only state this double exists to reach
            // (`[[feedback_then_some_eager]]`).
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n > 0).then(|| n - 1)
            })
            .is_ok()
    }

    fn refuse() -> StoreError {
        StoreError::Io(std::io::Error::other("the disk said no (FailingStore)"))
    }
}

/// Every read delegates and every write asks first.
///
/// Reads are **never** failed, deliberately: the point of the double is to let
/// a test compare what the app believes against what actually landed, and a
/// store that cannot be read cannot answer that.
impl Store for FailingStore {
    fn put_episode(&mut self, episode: &Episode) -> StoreResult<()> {
        if self.reject_put.load(Ordering::SeqCst) {
            return Err(Self::refuse());
        }
        self.inner.put_episode(episode)
    }

    fn rewrite_episode(&mut self, episode: &Episode, own: &[Turn]) -> StoreResult<()> {
        // Spends the append budget, because a rewrite IS the write this double
        // exists to fail: turn deletion is the one path where a store error
        // must leave history intact, and a test cannot check that without
        // being able to make the write fail.
        if !self.allow_append() {
            return Err(Self::refuse());
        }
        self.inner.rewrite_episode(episode, own)
    }

    fn append_turn(&mut self, episode: &EpisodeId, turn: &Turn) -> StoreResult<()> {
        if !self.allow_append() {
            return Err(Self::refuse());
        }
        self.inner.append_turn(episode, turn)
    }

    fn append_slice(
        &mut self,
        episode: &EpisodeId,
        source: &EpisodeId,
        start: usize,
        end: usize,
    ) -> StoreResult<()> {
        // A slice is an append: it consumes the same budget, because a test
        // about "the write failed" must not be quietly satisfied by a write
        // that took a different method to disk.
        if !self.allow_append() {
            return Err(Self::refuse());
        }
        self.inner.append_slice(episode, source, start, end)
    }

    fn load_episode(&self, id: &EpisodeId) -> StoreResult<LoadedEpisode> {
        self.inner.load_episode(id)
    }

    // The inner store's lazy reads, not the trait's load-everything defaults:
    // a test on this double must meet the same unloaded bodies as one on the
    // store it wraps.
    fn load_structure(&self) -> StoreResult<crate::graph::Graph> {
        self.structure_loads.fetch_add(1, Ordering::SeqCst);
        self.inner.load_structure()
    }

    fn load_bodies(
        &self,
        ids: &[crate::graph::TurnId],
    ) -> StoreResult<Vec<(crate::graph::TurnId, Vec<crate::message::Message>)>> {
        self.inner.load_bodies(ids)
    }

    fn list_episodes(&self) -> StoreResult<Vec<EpisodeId>> {
        self.inner.list_episodes()
    }

    fn episode_len(&self, id: &EpisodeId) -> StoreResult<Option<usize>> {
        self.inner.episode_len(id)
    }

    fn delete_episode(&mut self, id: &EpisodeId) -> StoreResult<()> {
        let refused = self
            .refused_deletes
            .lock()
            .is_ok_and(|ids| ids.contains(id));
        if refused {
            return Err(Self::refuse());
        }
        self.inner.delete_episode(id)
    }

    fn put_ref(&mut self, name: &str, episode: &EpisodeId) -> StoreResult<()> {
        self.inner.put_ref(name, episode)
    }

    fn get_ref(&self, name: &str) -> StoreResult<Option<EpisodeId>> {
        self.inner.get_ref(name)
    }
}

#[cfg(test)]
mod tests {
    use super::FailingStore;

    // Told to fail nothing, the double is a store: every write it lets
    // through lands as the store it wraps would keep it, which is what a
    // test comparing belief with what landed relies on.
    crate::store::contract::contract_tests!(FailingStore::refusing_deletes().0);
}
