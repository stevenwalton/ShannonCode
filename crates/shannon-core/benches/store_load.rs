//! Bench guard: **episode load** — the startup path (end-of-M3 gate).
//!
//! `JsonlStore::load_episode` scans the JSONL log, re-verifies every turn's
//! content hash (the integrity check that earns keeping ids on disk), and
//! rebuilds the episode; `load_graph` does that for every episode in the
//! store. Both are O(history) *by design* — they run at startup and when
//! `:graph`/id-completion snapshots, never per frame — but O(history) paths
//! are exactly the ones that rot silently as histories grow. This guard
//! exists so a regression (accidental quadratic re-parse, a per-turn fsync,
//! hashing the same bytes twice) shows up as a number, not as a laggy launch
//! two months from now. The parked lazy/tail-first loading work
//! (`Milestone 3.md` watch-item) starts from these numbers when a bench —
//! not a hunch — says it's time.
//!
//! Run with `cargo bench -p shannon-core` (the user drives bench runs and
//! reads the tails; see AGENTS.md conventions).

use std::time::SystemTime;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use shannon_core::{Episode, EpisodeId, JsonlStore, Message, Store, Turn, TurnConfig};
use tempfile::TempDir;

fn cfg() -> TurnConfig {
    TurnConfig {
        model: "claude-haiku".into(),
        max_tokens: None,
        effort: None,
    }
}

/// A sealed 2-message turn with ~realistic content sizes (a short prompt, a
/// few-hundred-char reply) so the hash re-verification does representative
/// work — benching empty strings would flatter the load path.
fn turn(i: usize) -> Turn {
    let user = format!("prompt {i}: please explain the thing about the code");
    let assistant = format!(
        "reply {i}: {}",
        "the explanation goes on a bit. ".repeat(12)
    );
    Turn::seal(
        vec![Message::user(user), Message::assistant(assistant)],
        cfg(),
        SystemTime::UNIX_EPOCH,
    )
}

/// A store holding one root episode with `n` turns (the long-conversation
/// shape) — `load_episode`'s worst case is depth, not breadth.
fn store_with_turns(dir: &TempDir, n: usize) -> (JsonlStore, EpisodeId) {
    let mut store = JsonlStore::new(dir.path()).expect("open store");
    let id = EpisodeId::from_seed(b"bench-root");
    store
        .put_episode(&Episode::root(id, cfg()))
        .expect("put episode");
    for i in 0..n {
        store.append_turn(&id, &turn(i)).expect("append turn");
    }
    (store, id)
}

/// Startup's real call: rebuild the WHOLE graph (every episode's log scanned
/// and integrity-checked). One episode deep vs. many episodes wide are the
/// two axes; depth is the one that grows without bound in normal use.
fn bench_load_graph(c: &mut Criterion) {
    let mut group = c.benchmark_group("load_graph");
    for n in [100usize, 1_000, 5_000] {
        let dir = TempDir::new().unwrap();
        let (store, _) = store_with_turns(&dir, n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| black_box(s.load_graph().expect("load graph")));
        });
    }
    group.finish();
}

/// The single-episode slice of the same path (what `:open`/resume touch).
fn bench_load_episode(c: &mut Criterion) {
    let mut group = c.benchmark_group("load_episode");
    for n in [100usize, 1_000, 5_000] {
        let dir = TempDir::new().unwrap();
        let (store, id) = store_with_turns(&dir, n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| black_box(s.load_episode(&id).expect("load episode")));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_load_graph, bench_load_episode);
criterion_main!(benches);
