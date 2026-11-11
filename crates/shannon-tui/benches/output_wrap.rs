//! Benchmark for the output-pane render hot path.
//!
//! The renderer calls `Output::visible_window(viewport, shut, width, height)`
//! once per frame — `shut` a window's folds under the shipped rule, as a
//! frame asks (N5a). The M1 baseline (recorded in the lib-split commit and the M2
//! doc) re-wrapped the *entire* buffer on every call, so per-frame cost
//! climbed O(history): 256 lines ~88us up to 16384 lines ~5.7ms.
//!
//! The M2 `Output -> Block` wrap cache retired that: a steady-state frame
//! at an unchanged width re-wraps nothing and only slices the cached
//! rows, skipping blocks above the viewport. So this now measures the
//! *cached* frame cost, which should be (a) far lower than the baseline
//! and (b) roughly **flat across history size** — that flatness is the
//! O(viewport) property the cache buys. (Criterion's setup builds the
//! transcript outside the timed loop; the first timed call warms the
//! cache, the rest are pure cache hits — i.e. exactly a redraw.)
//!
//! The bench is phrased against the public operation (build a transcript,
//! ask for a viewport's worth of rows) so it's the *same* bench as the
//! baseline — the before/after is honest, only the internals changed.
//!
//! The synthetic transcript is deterministic (a tiny LCG, no rng dep) so
//! numbers are reproducible run-to-run. Line lengths are mixed — most
//! lines fit the width, ~1 in 3 overflow and wrap into 2–4 visual rows —
//! to approximate real chat/code output rather than uniform short lines.

use criterion::{BatchSize, BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use shannon_tui::output::{FoldRule, Folds, Output};
use shannon_tui::viewport::Viewport;

/// Deterministic pseudo-random byte stream (LCG) — just enough variety
/// to make line lengths and content non-uniform without pulling in a
/// dependency or sacrificing reproducibility.
struct Lcg(u64);
impl Lcg {
    fn next_u32(&mut self) -> u32 {
        // glibc's constants; we only need spread, not statistical quality.
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    fn in_range(&mut self, lo: usize, hi: usize) -> usize {
        lo + (self.next_u32() as usize) % (hi - lo)
    }
}

/// Build an `Output` holding `n_lines` of synthetic transcript via the
/// streaming turn API (one sealed assistant block of `n_lines` lines).
/// Roughly one line in three is long enough to wrap at a typical width.
fn build_output(n_lines: usize) -> Output {
    const WORDS: &[&str] = &[
        "fn", "let", "match", "the", "render", "loop", "wraps", "every", "line", "to", "width",
        "which", "is", "the", "cost", "we", "cache", "buffer", "viewport", "stream", "chunk",
        "terminal", "ratatui",
    ];
    let mut rng = Lcg(0x5EED_C0DE_1234_0001);
    let mut out = Output::new();
    out.begin_turn("benchmark");
    for i in 0..n_lines {
        // ~1 in 3 lines is "long" (wraps); the rest are short.
        let target = if i % 3 == 0 {
            rng.in_range(120, 320)
        } else {
            rng.in_range(8, 70)
        };
        let mut line = String::with_capacity(target + 1);
        while line.len() < target {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(WORDS[rng.in_range(0, WORDS.len())]);
        }
        line.push('\n'); // each chunk is its own logical line
        out.append_chunk(&line);
    }
    out.end_turn();
    out
}

fn bench_visible_window(c: &mut Criterion) {
    // A realistic content width and a tall pane: the wrap cost is paid
    // over the whole history regardless of how few rows are shown.
    const WIDTH: u16 = 100;
    const HEIGHT: u16 = 50;

    let folds = Folds::default();
    let shut = folds.shut(FoldRule::SHIPPED);
    let mut group = c.benchmark_group("visible_window_follow_tail");
    for &n in &[256usize, 1_024, 4_096, 16_384] {
        let output = build_output(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &output, |b, o| {
            b.iter(|| {
                black_box(o.visible_window(
                    &Viewport::default(),
                    &shut,
                    black_box(WIDTH),
                    black_box(HEIGHT),
                ))
            });
        });
    }
    group.finish();
}

/// An `Output` with `n_turns` sealed history turns (each ~20 lines) plus an
/// open `open_lines`-line streaming block, cache primed. Deterministic content.
fn build_with_history(n_turns: usize, open_lines: usize) -> Output {
    let mut out = Output::new();
    for _ in 0..n_turns {
        out.begin_turn("user turn");
        for i in 0..20 {
            out.append_chunk(&format!(
                "assistant reply line {i} with enough words here to wrap\n"
            ));
        }
        out.end_turn();
    }
    // The in-flight (unsealed) block being streamed into — a fenced *code*
    // block, because a prose one takes a cheaper path and would hide the cost.
    out.begin_turn("streaming");
    out.append_chunk("```rust\n");
    for i in 0..open_lines {
        out.append_chunk(&format!(
            "    let value_{i} = compute(i, {i}); // streamed code line that wraps long\n"
        ));
    }
    // Warm the cache (incl. styling + syntect).
    let folds = Folds::default();
    let _ = out.visible_window(
        &Viewport::default(),
        &folds.shut(FoldRule::SHIPPED),
        100,
        50,
    );
    out
}

/// Append-during-stream re-render with `N` history turns behind the open block.
/// Each timed op appends one chunk and re-renders; `iter_batched` clones a fresh
/// primed transcript per batch. The cost should be O(viewport + open block) and
/// **flat across N** — history is cached, so a long session doesn't slow
/// streaming.
///
/// **This bench does NOT watch the re-highlight cost**, though its doc comment
/// used to claim it did ("the bench to watch if syntect re-highlighting of a
/// growing open code block ever needs to go incremental"). It varies history —
/// the flat axis — and pins the open block at 200 lines, so the quadratic term
/// is a constant in every measurement it takes. It reported 28.7 / 29.0 / 30.2 ms
/// across N=64/256/1024 while a streaming reply cost 25 s of CPU to render.
/// [`bench_append_by_open_length`] is the one that varies the axis that moves.
fn bench_append_during_stream(c: &mut Criterion) {
    const WIDTH: u16 = 100;
    const HEIGHT: u16 = 50;

    let folds = Folds::default();
    let shut = folds.shut(FoldRule::SHIPPED);
    let mut group = c.benchmark_group("append_during_stream");
    for &n in &[16usize, 64, 256, 1_024] {
        let base = build_with_history(n, 200);
        group.bench_with_input(BenchmarkId::from_parameter(n), &base, |b, base| {
            b.iter_batched(
                || base.clone(),
                |mut o| {
                    // Extend the open *code* fence so the re-render re-highlights it.
                    o.append_chunk("    let next = compute(i, i + 1); // another code line\n");
                    black_box(o.visible_window(
                        &Viewport::default(),
                        &shut,
                        black_box(WIDTH),
                        black_box(HEIGHT),
                    ));
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

/// The axis that actually moves: cost of one append+re-render as a function of
/// **how long the open block already is**, with history pinned.
///
/// Every chunk invalidates the open block's cache, so the next render re-styles
/// its whole length — cost per chunk is O(L) and cost per turn is O(L²). With
/// syntect in that loop the constant was ~45×. A doubling of L should now cost
/// about double, not about quadruple, and a regression here is a streaming
/// reply that takes seconds of CPU to draw.
fn bench_append_by_open_length(c: &mut Criterion) {
    const WIDTH: u16 = 100;
    const HEIGHT: u16 = 50;

    let folds = Folds::default();
    let shut = folds.shut(FoldRule::SHIPPED);
    let mut group = c.benchmark_group("append_by_open_length");
    for &lines in &[50usize, 200, 800, 3_200] {
        let base = build_with_history(16, lines);
        group.bench_with_input(BenchmarkId::from_parameter(lines), &base, |b, base| {
            b.iter_batched(
                || base.clone(),
                |mut o| {
                    o.append_chunk("    let next = compute(i, i + 1); // another code line\n");
                    black_box(o.visible_window(
                        &Viewport::default(),
                        &shut,
                        black_box(WIDTH),
                        black_box(HEIGHT),
                    ));
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_visible_window,
    bench_append_during_stream,
    bench_append_by_open_length
);
criterion_main!(benches);
