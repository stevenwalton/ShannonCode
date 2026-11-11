//! How good is `ctx`'s estimate? Measured against the windows a store holds.
//!
//! Every turn a session produced ITSELF (an `Own` occurrence) carries the
//! window its final request measured (`last_round`) on the history before
//! it. This predicts each such window two ways — exactly as the app does
//! when a window has to be estimated (`shannon_core::window`) — and reports
//! the error:
//!
//! - `text`: the history's characters at four a token, nothing else.
//! - `anchored`: a window this history measured earlier, plus the text
//!   added since — the app's estimate when an earlier turn measured the
//!   same history (`App::render_session`).
//!
//! In two scenarios:
//!
//! - `forward`: each measured turn from the one before it — a history that
//!   grew a little (an edit, a deleted turn).
//! - `shrink`: from the session's LAST measured turn, each earlier one whose
//!   history is at most a quarter as long — what a `:compact` asks: an
//!   estimate for a much shorter history than the one last measured.
//!
//! Claude Code turns (`claude-code/…`) sealed before 10-15 carry the SUM of
//! their requests, not a window, so they are counted and left out; so is a
//! turn sealed before `last_round` existed.
//!
//! Takes store directories, ships no data:
//!
//! ```text
//! cargo run -p shannon-core --example ctx_estimate -- ~/.local/state/shannon/projects/*
//! ```

use std::collections::HashSet;
use std::path::Path;

use shannon_core::JsonlStore;
use shannon_core::graph::Turn;
use shannon_core::store::Store;
use shannon_core::window;

/// One turn's measurement: its history's characters and the window measured.
#[derive(Clone, Copy)]
struct Point {
    chars: usize,
    window: u32,
}

#[derive(Default)]
struct Errors {
    text: Vec<f64>,
    anchored: Vec<f64>,
}

impl Errors {
    /// Score a prediction of `at` from the measurement `from`.
    fn score(&mut self, from: Point, at: Point) {
        let truth = f64::from(at.window);
        let text = f64::from(window::tokens(at.chars));
        let anchored = f64::from(window::tokens(at.chars))
            + f64::from(window::overhead(from.window, from.chars));
        self.text.push((text - truth) / truth);
        self.anchored.push((anchored - truth) / truth);
    }
}

fn main() {
    let dirs: Vec<String> = std::env::args().skip(1).collect();
    if dirs.is_empty() {
        eprintln!("usage: ctx_estimate STORE_DIR…   (~/.local/state/shannon/projects/*)");
        std::process::exit(2);
    }
    let (mut forward, mut shrink) = (Errors::default(), Errors::default());
    let (mut sessions, mut claude_code, mut unmeasured) = (0usize, 0usize, 0usize);
    for dir in &dirs {
        let graph = match JsonlStore::new(Path::new(dir)).and_then(|s| s.load_graph()) {
            Ok(graph) => graph,
            Err(e) => {
                eprintln!("{dir}: skipped ({e})");
                continue;
            }
        };
        for id in graph.episode_ids() {
            let Some(episode) = graph.episode(&id) else {
                continue;
            };
            let own: HashSet<_> = episode.own_turns().into_iter().collect();
            let Some(turns) = graph.records(&id) else {
                continue;
            };
            let points = measured(&turns, &own, &mut claude_code, &mut unmeasured);
            if points.is_empty() {
                continue;
            }
            sessions += 1;
            for pair in points.windows(2) {
                forward.score(pair[0], pair[1]);
            }
            let last = points[points.len() - 1];
            for &earlier in &points[..points.len() - 1] {
                if earlier.chars * 4 <= last.chars {
                    shrink.score(last, earlier);
                }
            }
        }
    }
    println!(
        "{sessions} sessions; left out: {claude_code} Claude Code turns, \
         {unmeasured} with no window recorded"
    );
    println!("relative error (estimate − measured) / measured; negative = too low\n");
    println!(
        "{:<10} {:<9} {:>6} {:>9} {:>9} {:>9}",
        "scenario", "estimate", "n", "median", "med |e|", "p90 |e|"
    );
    for (scenario, errors) in [("forward", &forward), ("shrink", &shrink)] {
        for (estimate, values) in [("text", &errors.text), ("anchored", &errors.anchored)] {
            report(scenario, estimate, values);
        }
    }
}

/// The windows `turns`' own turns measured, oldest first, each with its
/// history's characters — counting what is left out.
fn measured(
    turns: &[Turn],
    own: &HashSet<shannon_core::graph::TurnId>,
    claude_code: &mut usize,
    unmeasured: &mut usize,
) -> Vec<Point> {
    let mut chars = 0usize;
    let mut points = Vec::new();
    for turn in turns {
        chars += window::turn_chars(turn);
        if !own.contains(&turn.id) {
            continue;
        }
        if turn.provenance.config.model.starts_with("claude-code/") {
            *claude_code += 1;
            continue;
        }
        let Some(round) = turn.provenance.last_round else {
            *unmeasured += 1;
            continue;
        };
        let window = round.prompt_tokens().saturating_add(round.output_tokens);
        if window > 0 {
            points.push(Point { chars, window });
        }
    }
    points
}

fn report(scenario: &str, estimate: &str, values: &[f64]) {
    if values.is_empty() {
        println!("{scenario:<10} {estimate:<9} {:>6}", 0);
        return;
    }
    let mut signed = values.to_vec();
    signed.sort_by(f64::total_cmp);
    let mut abs: Vec<f64> = values.iter().map(|v| v.abs()).collect();
    abs.sort_by(f64::total_cmp);
    let at = |v: &[f64], q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    println!(
        "{scenario:<10} {estimate:<9} {:>6} {:>8.1}% {:>8.1}% {:>8.1}%",
        values.len(),
        100.0 * at(&signed, 0.5),
        100.0 * at(&abs, 0.5),
        100.0 * at(&abs, 0.9),
    );
}
