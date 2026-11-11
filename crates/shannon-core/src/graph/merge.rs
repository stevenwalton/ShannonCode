//! **Graph surgery** — composing a new episode from pieces of existing ones
//! (design.md §1.7). A merge, a squash, a prune, a cherry-pick, a prepend — and
//! whatever a user invents — are all the *same* operation here: assemble an ordered
//! list of [`Piece`]s and lower it with [`Graph::compose`].
//!
//! The bet (and the whole reason the piece table exists): "attaching a point of the
//! graph to a point of the graph" is not many operations, it is one. A [`Piece`] is
//! either a **span** borrowed by reference from an existing episode or a run of
//! **new** authored turns; a composition is a `Vec<Piece>`. So:
//!
//! ```text
//! append   = [Span(E, 0..len), New([turn])]
//! prepend  = [New([root]),     Span(E, 0..len)]              (attach *behind* a node)
//! merge    = [Span(base,0..k), Span(branch,k..), Span(base,k..)]   (attach *between* nodes)
//! squash   = [Span(base,0..k), New([summary]), Span(base,k+n..)]
//! prune    = [Span(E,0..i),    Span(E,j..len)]               (drop [i, j))
//! ```
//!
//! Merge-straight-in and squash differ only in their *middle* piece (a branch span
//! vs. a new summary) — so building the merge engine builds the squash engine too.
//!
//! Strategies are **pure**: [`MergeStrategy::plan`] takes `&Graph` and returns the
//! plan; [`Graph::compose`] does every mutation (inserting new turns, projecting
//! spans to their data owners, splicing). A strategy therefore can't corrupt the
//! store and is trivial to unit-test — which is exactly what lets the environment
//! invite users to write their own merge/prune/pick strategies, however silly.

use std::collections::HashMap;

use crate::graph::episode::{EpisodeId, Origin, Segment};
use crate::graph::graph::{Graph, Placed};
use crate::graph::turn::{Turn, TurnConfig, TurnId};

/// One piece of a composed episode. Either a **span** of an existing episode's
/// derived sequence — borrowed by reference, never copied — or a run of **new**
/// authored turns (the only place a composition introduces new data: a squash
/// summary, a prepended preamble). A `Vec<Piece>` is the universal recipe every
/// graph-surgery strategy produces; [`Graph::compose`] lowers it.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    /// Turns `[start, end)` of `episode`'s derived sequence, shared by reference.
    /// Lowered via [`Graph::project`], so it references the true data owners (never a
    /// pass-through intermediate) and costs nothing to store. Out-of-range bounds
    /// clamp, like every other range in the model.
    Span {
        episode: EpisodeId,
        start: usize,
        end: usize,
    },
    /// Newly-authored turns to insert and own — the composition's only new data.
    New(Vec<Turn>),
}

/// Why a [`MergeStrategy`] couldn't produce a plan. Strategies validate their own
/// preconditions (an input episode that isn't in the graph, a shape that doesn't
/// fit); the [`compose`](Graph::compose) engine itself is total (it clamps ranges),
/// so all fallibility lives at the planning layer, where it's cheap to test.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MergeError {
    /// A strategy referenced an episode the graph doesn't contain.
    #[error("merge: episode not found: {0}")]
    EpisodeNotFound(String),
    /// A strategy's inputs don't fit its shape (e.g. two lines of history that don't
    /// share a base). The payload explains what didn't line up.
    #[error("merge: {0}")]
    Unmergeable(String),
}

/// A **composition strategy**: given the graph (read-only) and the strategy's own
/// inputs (its fields), produce the ordered [`Piece`]s of a NEW episode — a merge, a
/// squash, a prune, a cherry-pick, or something a user dreams up. Pure by design: no
/// mutation happens here (the [`compose`](Graph::compose) engine does it all), so a
/// strategy is easy to test in isolation and physically cannot corrupt the store.
///
/// This is the environment's extension point. A built-in strategy and a user's
/// experimental one are the same kind of thing — implement `plan`, return pieces.
pub trait MergeStrategy {
    /// Read the graph and return the pieces of the composed result, or a
    /// [`MergeError`] if the inputs don't fit.
    fn plan(&self, graph: &Graph) -> Result<Vec<Piece>, MergeError>;
}

/// Guard that `id` is in `graph`, or a [`MergeError::EpisodeNotFound`] with its
/// hex — the precondition nearly every built-in strategy opens with. One helper
/// so the check reads the same everywhere and a new strategy can't spell it
/// subtly differently. (Crate-internal; an out-of-crate strategy writes the
/// two-line guard itself, as the TUI's `ResultInject` does.)
fn require(graph: &Graph, id: &EpisodeId) -> Result<(), MergeError> {
    if graph.episode(id).is_none() {
        return Err(MergeError::EpisodeNotFound(id.to_hex()));
    }
    Ok(())
}

impl Graph {
    /// Length of the longest common **prefix** of `a`'s and `b`'s derived sequences:
    /// the positions that are the same OCCURRENCE in both — shared history — compared
    /// by position. This is *where two lines of history diverge*, the point a merge
    /// splices around; the prefix half of a 3-way merge base.
    ///
    /// Equal content is not shared history. Two sessions that each sent `hi` and got
    /// the same reply hold one `TurnId` and two productions, and splicing around them
    /// as if they were one would drop a turn from the merge
    /// (`[[content-addressed-ids]]`).
    pub fn common_prefix_len(&self, a: &EpisodeId, b: &EpisodeId) -> usize {
        let pa = self.placed(a);
        let pb = self.placed(b);
        pa.iter()
            .zip(&pb)
            .take_while(|(x, y)| x.same_occurrence(y))
            .count()
    }

    /// **Reconstruct** the minimal `Vec<Piece>` that reproduces the desired `target`
    /// turn sequence, sharing every turn that already exists ANYWHERE in the graph by
    /// reference (a `Span`, canonicalized to its true owner by
    /// [`project`](Self::project) when composed) and owning only genuinely-new turns (a
    /// `New` run). This is the universal edit primitive (design.md §1.2): whether the
    /// change is at the front (re-root), the middle (branch), the back (append), or
    /// scattered — and whether an unchanged turn came from *this* episode or another
    /// branch — falls out of where the known/new boundaries land.
    ///
    /// An unchanged turn is recognized as its existing node when it is the same
    /// **production** — same content AND same provenance — never by content alone: a
    /// result turn returned from a child holds the child's exact messages, and sharing
    /// the child's occurrence in its place would drop what the result distilled
    /// (`[[content-addressed-ids]]`). Equal content with other facts is a new
    /// occurrence; its messages are still stored once. That is the consistency check
    /// the whole approach rests on: `compose` of the result yields exactly `target`,
    /// provenance included. Adjacent turns from the same source at consecutive
    /// positions coalesce into one span, so the piece list stays minimal.
    ///
    /// `O(N)` in the total turns across all episodes (the index is built per call — an
    /// edit-time action, not a per-frame hot path; an incremental index is a later
    /// optimization).
    pub fn reconstruct(&self, target: &[Turn]) -> Vec<Piece> {
        // Each episode's placed sequence, plus every location (episode, pos) per known
        // turn id. `project` resolves any location to the true owner, so which of the
        // same production's locations we pick only affects span *coalescing*.
        let derived: HashMap<EpisodeId, Vec<Placed<'_>>> = self
            .episode_ids()
            .into_iter()
            .map(|id| (id, self.placed(&id)))
            .collect();
        let mut loc: HashMap<TurnId, Vec<(EpisodeId, usize)>> = HashMap::new();
        for (ep, seq) in &derived {
            for (i, p) in seq.iter().enumerate() {
                loc.entry(p.turn).or_default().push((*ep, i));
            }
        }
        // Is the turn at `(ep, i)` this very production?
        let is = |turn: &Turn, ep: &EpisodeId, i: usize| {
            derived
                .get(ep)
                .and_then(|seq| seq.get(i))
                .is_some_and(|p| p.turn == turn.id && *p.provenance == turn.provenance)
        };

        let mut pieces: Vec<Piece> = Vec::new();
        // Invariant: at most one of `span` / `new_run` is ever non-empty — each is
        // emitted before the other opens. Two sinks take the buffers as params (not
        // captures), so the loop can keep mutating them.
        let mut span: Option<(EpisodeId, usize, usize)> = None; // (episode, start, end)
        let mut new_run: Vec<Turn> = Vec::new();
        let emit_span = |span: &mut Option<(EpisodeId, usize, usize)>, pieces: &mut Vec<Piece>| {
            if let Some((episode, start, end)) = span.take() {
                pieces.push(Piece::Span {
                    episode,
                    start,
                    end,
                });
            }
        };
        let emit_new = |new_run: &mut Vec<Turn>, pieces: &mut Vec<Piece>| {
            if !new_run.is_empty() {
                pieces.push(Piece::New(std::mem::take(new_run)));
            }
        };

        for turn in target {
            // Extend the current span if this turn continues it (same source, next pos).
            if let Some((ep, start, end)) = span
                && is(turn, &ep, end)
            {
                span = Some((ep, start, end + 1));
                continue;
            }
            let known = loc
                .get(&turn.id)
                .and_then(|at| at.iter().find(|(ep, i)| is(turn, ep, *i)));
            match known {
                // A known node that doesn't extend the run → close both buffers, open a span.
                Some(&(episode, idx)) => {
                    emit_span(&mut span, &mut pieces);
                    emit_new(&mut new_run, &mut pieces);
                    span = Some((episode, idx, idx + 1));
                }
                // A genuinely-new turn → close any open span, then own it.
                None => {
                    emit_span(&mut span, &mut pieces);
                    new_run.push(turn.clone());
                }
            }
        }
        emit_span(&mut span, &mut pieces);
        emit_new(&mut new_run, &mut pieces);
        pieces
    }

    /// Lower a composition into a new episode — the one engine every strategy flows
    /// through. Each [`Piece::Span`] is [`project`](Self::project)ed (shared by
    /// reference, owner-flattened); each [`Piece::New`] run is inserted and
    /// contributes an [`Segment::Own`]. Then [`splice`](Self::splice). The source
    /// episodes are untouched — a composition only ever *reads* them.
    ///
    /// Total: out-of-range spans clamp (like `project`), so a strategy can't make
    /// `compose` fail — it can only produce a different (still valid) result.
    pub fn compose(
        &mut self,
        new_id: EpisodeId,
        config: TurnConfig,
        pieces: Vec<Piece>,
        origin: Option<Origin>,
    ) -> EpisodeId {
        let mut segments: Vec<Segment> = Vec::new();
        for piece in pieces {
            match piece {
                Piece::Span {
                    episode,
                    start,
                    end,
                } => segments.extend(self.project(&episode, start, end)),
                Piece::New(turns) => {
                    let own: Vec<_> = turns.into_iter().map(|t| self.admit(t)).collect();
                    if !own.is_empty() {
                        segments.push(Segment::Own(own));
                    }
                }
            }
        }
        self.splice(new_id, config, segments, origin)
    }

    /// Run a strategy end to end: [`plan`](MergeStrategy::plan) → [`compose`]. Every
    /// merge/prune/pick/squash — built-in or user-defined — is invoked this way, so
    /// the engine treats them all identically.
    ///
    /// [`compose`]: Self::compose
    pub fn apply_strategy(
        &mut self,
        strategy: &dyn MergeStrategy,
        new_id: EpisodeId,
        config: TurnConfig,
        origin: Option<Origin>,
    ) -> Result<EpisodeId, MergeError> {
        let pieces = strategy.plan(self)?;
        Ok(self.compose(new_id, config, pieces, origin))
    }
}

/// **Merge a branch's divergent turns straight into its base** at the fork point
/// (design.md §1.7): `[base 0..k] ++ [branch k..] ++ [base k..]`, where `k` is where
/// the two diverge ([`common_prefix_len`](Graph::common_prefix_len)). Base
/// `A→B→C` with a branch off `B` of `X→Y→Z` composes to `A→B→X→Y→Z→C`. Nothing is
/// deleted or authored — the branch's span is simply re-attached *between* two base
/// spans (the foundation for intra-node insertion). The originals are preserved.
///
/// Intended for an **append-branch** (one that shares a prefix with the base and
/// then adds turns): there, `k` is exactly the fork point. If the branch *edited* a
/// shared turn, `k` falls before that edit and the result keeps both versions — a
/// valid but unusual "straight in"; richer edit-aware merges are a later strategy.
pub struct MergeStraightIn {
    pub base: EpisodeId,
    pub branch: EpisodeId,
}

impl MergeStrategy for MergeStraightIn {
    fn plan(&self, graph: &Graph) -> Result<Vec<Piece>, MergeError> {
        require(graph, &self.base)?;
        require(graph, &self.branch)?;
        let base_len = graph.full_turns(&self.base).len();
        let branch_len = graph.full_turns(&self.branch).len();
        let k = graph.common_prefix_len(&self.base, &self.branch);
        Ok(vec![
            Piece::Span {
                episode: self.base,
                start: 0,
                end: k,
            },
            Piece::Span {
                episode: self.branch,
                start: k,
                end: branch_len,
            },
            Piece::Span {
                episode: self.base,
                start: k,
                end: base_len,
            },
        ])
    }
}

/// **Prepend** new turns before a target's whole history — the ergonomic name for
/// [`ReRoot`] with `drop == 0` (attach a root, keep *everything*). Kept as a
/// self-documenting alias (`Prepend { target, root }` reads better than a `drop: 0`);
/// the piece-construction lives once, in `ReRoot`.
pub struct Prepend {
    pub target: EpisodeId,
    pub root: Vec<Turn>,
}

impl MergeStrategy for Prepend {
    fn plan(&self, graph: &Graph) -> Result<Vec<Piece>, MergeError> {
        ReRoot {
            episode: self.target,
            drop: 0,
            root: self.root.clone(),
        }
        .plan(graph)
    }
}

/// **Squash** a contiguous range of an episode into a single authored summary
/// (design.md §1.7): `[E 0..start] ++ [summary] ++ [E end..len]`. Squashing
/// positions `2..5` (say `X,Y,Z`) of `A→B→X→Y→Z→C` into `P` ⇒ `A→B→P→C`. The
/// squashed turns are simply not referenced; only the summary is new data. A new
/// session — the original is preserved. An empty range (`start == end`) squashes
/// nothing and just *inserts* the summary there.
///
/// The `summary` is an **input** (authored turns), not generated here — keeping
/// `plan` pure. Summary *generation* (e.g. asking the model to summarize the range)
/// is a separate, async step that produces `summary` before this strategy runs, so
/// the strategy stays a total, testable function whether the summary was typed or
/// machine-written.
pub struct Squash {
    pub episode: EpisodeId,
    pub start: usize,
    pub end: usize,
    pub summary: Vec<Turn>,
}

impl MergeStrategy for Squash {
    fn plan(&self, graph: &Graph) -> Result<Vec<Piece>, MergeError> {
        require(graph, &self.episode)?;
        let len = graph.full_turns(&self.episode).len();
        // Validate rather than clamp: squashing the *wrong* range would silently drop
        // or keep the wrong turns, which is exactly the data-loss we won't tolerate.
        if self.start > self.end || self.end > len {
            return Err(MergeError::Unmergeable(format!(
                "squash range {}..{} out of bounds (len {len})",
                self.start, self.end
            )));
        }
        Ok(vec![
            Piece::Span {
                episode: self.episode,
                start: 0,
                end: self.start,
            },
            Piece::New(self.summary.clone()),
            Piece::Span {
                episode: self.episode,
                start: self.end,
                end: len,
            },
        ])
    }
}

/// **Squash-merge** a branch into its base: replace the branch's divergent turns
/// with a single summary and splice it in at the fork point —
/// `[base 0..k] ++ [summary] ++ [base k..len]`, `k` = [`common_prefix_len`]. Base
/// `A→B→C` and a branch off `B` of `X→Y→Z` compose to `A→B→P→C` (with `P`
/// summarizing `X→Y→Z`). This is *exactly* [`MergeStraightIn`] with its middle piece
/// — the branch's span — swapped for a `New` summary: same shape, one piece
/// different. The originals are preserved; `branch` is read only to locate the fork
/// (and, when generation lands, to know what to summarize).
///
/// [`common_prefix_len`]: Graph::common_prefix_len
pub struct SquashMerge {
    pub base: EpisodeId,
    pub branch: EpisodeId,
    pub summary: Vec<Turn>,
}

impl MergeStrategy for SquashMerge {
    fn plan(&self, graph: &Graph) -> Result<Vec<Piece>, MergeError> {
        // Squash-merge IS a zero-range squash at the fork point: replacing the
        // empty range `[k, k)` of `base` with the summary yields exactly
        // `[base 0..k] ++ New ++ [base k..len]`. So delegate to `Squash` rather
        // than re-hand-roll the same triad (mirrors how `Prepend` delegates to
        // `ReRoot`). We still validate the branch — it's what locates `k`.
        require(graph, &self.branch)?;
        let k = graph.common_prefix_len(&self.base, &self.branch);
        Squash {
            episode: self.base,
            start: k,
            end: k,
            summary: self.summary.clone(),
        }
        .plan(graph)
    }
}

/// **Pick** — compose a new episode from an ordered list of spans cherry-picked from
/// existing episodes (one or several). The universal read-side composition (design.md
/// §1.2 base triad): a *prune* keeps the ranges around a dropped turn, a *sub-select*
/// keeps one range, and an N-way **gather** collects the best span from each of
/// `clone_n`'s variants (the model-variance researcher's payoff). Nothing is authored —
/// every turn is shared by reference (COW); the sources are untouched. Each pick is
/// `(episode, start, end)`; out-of-range bounds clamp via [`project`](Graph::project),
/// like every range in the model.
pub struct Pick {
    pub picks: Vec<(EpisodeId, usize, usize)>,
}

impl MergeStrategy for Pick {
    fn plan(&self, graph: &Graph) -> Result<Vec<Piece>, MergeError> {
        self.picks
            .iter()
            .try_for_each(|(episode, ..)| require(graph, episode))?;
        Ok(self
            .picks
            .iter()
            .map(|&(episode, start, end)| Piece::Span {
                episode,
                start,
                end,
            })
            .collect())
    }
}

/// **Re-root** — replace an episode's leading `drop` turns (its old root/context) with a
/// freshly authored `root`, keeping the rest: `[New(root), Span(episode, drop..len)]`
/// (design.md §1.2 base triad). The mirror of `branch`: `branch` shares the *prefix* and
/// diverges the *suffix*; `re-root` diverges the *prefix* and shares the *suffix*.
/// [`Prepend`] is the `drop == 0` case (attach a root, keep everything). Cheap because
/// turns are position-independent — no downstream rehash, unlike git rebase; the original
/// is preserved. Whether the kept suffix stays verbatim or is regenerated under the new
/// root is a content-source *mode*, not this plan.
pub struct ReRoot {
    pub episode: EpisodeId,
    /// How many leading turns of the old root to replace. `0` = pure prepend.
    pub drop: usize,
    pub root: Vec<Turn>,
}

impl MergeStrategy for ReRoot {
    fn plan(&self, graph: &Graph) -> Result<Vec<Piece>, MergeError> {
        require(graph, &self.episode)?;
        let len = graph.full_turns(&self.episode).len();
        // Validate rather than clamp: dropping more than exists means the caller has the
        // wrong point, which would silently discard the whole suffix.
        if self.drop > len {
            return Err(MergeError::Unmergeable(format!(
                "re-root drops {} turns but the episode has only {len}",
                self.drop
            )));
        }
        Ok(vec![
            Piece::New(self.root.clone()),
            Piece::Span {
                episode: self.episode,
                start: self.drop,
                end: len,
            },
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::episode::{EdgeKind, Episode};
    use crate::message::Message;
    use std::time::SystemTime;

    fn cfg() -> TurnConfig {
        TurnConfig {
            model: "claude-haiku".into(),
            max_tokens: None,
            effort: None,
        }
    }

    fn turn(tag: &str) -> Turn {
        Turn::seal(
            vec![Message::user(tag), Message::assistant(tag)],
            cfg(),
            SystemTime::UNIX_EPOCH,
        )
    }

    /// A root episode owning `tags` in order, inserted into a fresh graph.
    fn root_with(tags: &[&str]) -> (Graph, EpisodeId, Vec<Turn>) {
        let turns: Vec<Turn> = tags.iter().map(|t| turn(t)).collect();
        let id = EpisodeId::from_seed(b"base");
        let mut ep = Episode::root(id, cfg());
        let mut g = Graph::new();
        for t in &turns {
            g.insert_content(t.clone());
            ep.append(t.occurrence());
        }
        g.insert_episode(ep);
        (g, id, turns)
    }

    /// `reconstruct` shares an existing turn only when it is the SAME
    /// production. A result turn repeating a child's exact messages is a new
    /// occurrence — sharing the child's would hand it the child's facts and
    /// drop what it distilled. Its content is still stored once.
    #[test]
    fn reconstruct_shares_a_production_not_equal_content() {
        let (mut g, base, t) = root_with(&["a", "b"]);
        let mut b_result = t[1].clone();
        b_result.provenance.distilled_from = vec![t[0].id];
        assert_eq!(b_result.id, t[1].id, "premise: the same content");

        let same = vec![t[0].clone(), t[1].clone()];
        assert_eq!(
            g.reconstruct(&same),
            vec![Piece::Span {
                episode: base,
                start: 0,
                end: 2
            }],
            "the same productions are shared by reference"
        );
        let target = vec![t[0].clone(), b_result.clone()];
        assert_eq!(
            g.reconstruct(&target),
            vec![
                Piece::Span {
                    episode: base,
                    start: 0,
                    end: 1
                },
                Piece::New(vec![b_result.clone()]),
            ],
            "equal content with other facts is a new occurrence"
        );
        let out = EpisodeId::from_seed(b"out");
        g.compose(out, cfg(), g.reconstruct(&target), None);
        assert_eq!(
            g.records(&out),
            Some(target),
            "compose reproduces the target, facts included"
        );
        assert!(
            g.records(&base).unwrap()[1]
                .provenance
                .distilled_from
                .is_empty(),
            "the original keeps its own facts"
        );
    }

    fn ids(g: &Graph, ep: &EpisodeId) -> Vec<TurnId> {
        g.full_turns(ep)
    }

    #[test]
    fn compose_shares_spans_and_owns_only_new_turns() {
        // [Span(0..2), New(p), Span(2..3)] over [a,b,c] → [a, p, c]: a and c shared by
        // reference (owned by the base), only p is new data.
        let (mut g, base, t) = root_with(&["a", "b", "c"]);
        let p = turn("p");
        let out = EpisodeId::from_seed(b"out");
        g.compose(
            out,
            cfg(),
            vec![
                Piece::Span {
                    episode: base,
                    start: 0,
                    end: 2,
                },
                Piece::New(vec![p.clone()]),
                Piece::Span {
                    episode: base,
                    start: 2,
                    end: 3,
                },
            ],
            None,
        );
        // [a,b] ++ [p] ++ [c] = [a, b, p, c].
        assert_eq!(ids(&g, &out), vec![t[0].id, t[1].id, p.id, t[2].id]);
        assert_eq!(
            g.episode(&out).unwrap().own_turns(),
            vec![p.id],
            "only the new turn is owned; a/b/c stay shared"
        );
        // The base is untouched.
        assert_eq!(ids(&g, &base), vec![t[0].id, t[1].id, t[2].id]);
    }

    #[test]
    fn common_prefix_len_finds_the_divergence_point() {
        let (mut g, base, _t) = root_with(&["a", "b", "c"]);
        // A branch off b (shares [a,b]) that adds x,y,z.
        let branch = EpisodeId::from_seed(b"branch");
        g.branch_prefix(base, 2, branch);
        for tag in ["x", "y", "z"] {
            let t = turn(tag);
            g.insert_content(t.clone());
            g.episode_mut(&branch).unwrap().append(t.occurrence());
        }
        assert_eq!(g.common_prefix_len(&base, &branch), 2, "[a,b] shared");
        assert_eq!(
            g.common_prefix_len(&base, &base),
            3,
            "identical → full length"
        );
        let other = EpisodeId::from_seed(b"other");
        g.insert_episode(Episode::root(other, cfg()));
        let q = turn("q");
        g.insert_content(q.clone());
        g.episode_mut(&other).unwrap().append(q.occurrence());
        assert_eq!(g.common_prefix_len(&base, &other), 0, "disjoint roots");
    }

    #[test]
    fn common_prefix_len_counts_shared_occurrences_not_equal_content() {
        // Repeated identical content in SHARED history: a base [a,a,b] and a branch
        // that slices its [a,a] and diverges with c share two positions — compared
        // positionally, never collapsed by id.
        let a = turn("a");
        let (b, c) = (turn("b"), turn("c"));
        let mut g = Graph::new();
        for t in [&a, &b, &c] {
            g.insert_content(t.clone());
        }
        let base = EpisodeId::from_seed(b"base");
        let mut ep = Episode::root(base, cfg());
        for t in [&a, &a, &b] {
            ep.append(t.occurrence());
        }
        g.insert_episode(ep);
        let branch = EpisodeId::from_seed(b"branch");
        let mut ep = Episode::from_segments(
            branch,
            cfg(),
            vec![Segment::Slice {
                episode: base,
                start: 0,
                end: 2,
            }],
            None,
        );
        ep.append(c.occurrence());
        g.insert_episode(ep);
        assert_eq!(
            g.common_prefix_len(&base, &branch),
            2,
            "a,a shared positionally"
        );

        // The same content produced independently is NOT shared history: a second
        // root that also said [a,a,c] has a merge base of nothing with `base`.
        // (Until occurrences were split from content this read 2, and a merge
        // spliced around turns the two sessions never had in common.)
        let other = EpisodeId::from_seed(b"other");
        let mut ep = Episode::root(other, cfg());
        for t in [&a, &a, &c] {
            ep.append(t.occurrence());
        }
        g.insert_episode(ep);
        assert_eq!(
            g.common_prefix_len(&base, &other),
            0,
            "equal content, no shared history"
        );
    }

    #[test]
    fn merge_straight_in_splices_the_branch_at_the_fork() {
        // The canonical example: base a→b→c, branch off b is x→y→z, merged straight
        // in → a→b→x→y→z→c. All shared (no new data); base and branch untouched.
        let (mut g, base, t) = root_with(&["a", "b", "c"]);
        let branch = EpisodeId::from_seed(b"branch");
        g.branch_prefix(base, 2, branch); // shares [a,b]
        let (x, y, z) = (turn("x"), turn("y"), turn("z"));
        for t in [&x, &y, &z] {
            g.insert_content(t.clone());
            g.episode_mut(&branch).unwrap().append(t.occurrence());
        }

        let out = EpisodeId::from_seed(b"merged");
        let id = g
            .apply_strategy(&MergeStraightIn { base, branch }, out, cfg(), None)
            .unwrap();
        assert_eq!(
            ids(&g, &id),
            vec![t[0].id, t[1].id, x.id, y.id, z.id, t[2].id],
            "a b x y z c"
        );
        assert!(
            g.episode(&id).unwrap().own_turns().is_empty(),
            "a straight merge owns nothing — every turn is shared by reference"
        );
        // Originals preserved.
        assert_eq!(ids(&g, &base), vec![t[0].id, t[1].id, t[2].id]);
        assert_eq!(ids(&g, &branch), vec![t[0].id, t[1].id, x.id, y.id, z.id]);
    }

    #[test]
    fn merge_of_a_pure_append_branch_is_just_the_append() {
        // Branch shares ALL of the base then appends: k == base_len, so the middle base
        // span is empty and the result is base ++ appended. (Sharing the *whole* base
        // is a clone/tip-checkout shape, built directly — you can't `branch_prefix`
        // past the head, since a branch forks *at* an existing turn.)
        let (mut g, base, t) = root_with(&["a", "b"]);
        let branch = EpisodeId::from_seed(b"branch");
        let d = turn("d");
        g.insert_content(d.clone());
        g.insert_episode(Episode::from_segments(
            branch,
            cfg(),
            vec![
                Segment::Slice {
                    episode: base,
                    start: 0,
                    end: 2,
                },
                Segment::Own(vec![d.occurrence()]),
            ],
            Some(Origin {
                episode: base,
                turn: t[0].id,
                index: Some(0),
                kind: EdgeKind::Clone,
            }),
        ));
        assert_eq!(g.common_prefix_len(&base, &branch), 2, "shares all of base");

        let out = EpisodeId::from_seed(b"merged");
        g.apply_strategy(&MergeStraightIn { base, branch }, out, cfg(), None)
            .unwrap();
        assert_eq!(ids(&g, &out), vec![t[0].id, t[1].id, d.id], "a b d");
    }

    #[test]
    fn squash_falls_out_of_compose_with_no_new_engine() {
        // THE LITMUS: build the merged a→b→x→y→z→c, then squash x,y,z into a summary p
        // via the SAME compose engine → a→b→p→c. No squash-specific code exists yet.
        let (mut g, base, t) = root_with(&["a", "b", "c"]);
        let branch = EpisodeId::from_seed(b"branch");
        g.branch_prefix(base, 2, branch);
        let (x, y, z) = (turn("x"), turn("y"), turn("z"));
        for tt in [&x, &y, &z] {
            g.insert_content(tt.clone());
            g.episode_mut(&branch).unwrap().append(tt.occurrence());
        }
        let merged = EpisodeId::from_seed(b"merged");
        g.apply_strategy(&MergeStraightIn { base, branch }, merged, cfg(), None)
            .unwrap();
        // merged = [a,b,x,y,z,c]; squash positions 2..5 (x,y,z) into p.
        let p = turn("p");
        let squashed = EpisodeId::from_seed(b"squashed");
        g.compose(
            squashed,
            cfg(),
            vec![
                Piece::Span {
                    episode: merged,
                    start: 0,
                    end: 2,
                },
                Piece::New(vec![p.clone()]),
                Piece::Span {
                    episode: merged,
                    start: 5,
                    end: 6,
                },
            ],
            None,
        );
        assert_eq!(
            ids(&g, &squashed),
            vec![t[0].id, t[1].id, p.id, t[2].id],
            "a b p c — squash is just a New piece between two Spans"
        );
        assert_eq!(g.episode(&squashed).unwrap().own_turns(), vec![p.id]);
    }

    #[test]
    fn prepend_attaches_a_new_root_before_the_history() {
        let (mut g, base, t) = root_with(&["a", "b", "c"]);
        let p = turn("p");
        let out = EpisodeId::from_seed(b"prepended");
        g.apply_strategy(
            &Prepend {
                target: base,
                root: vec![p.clone()],
            },
            out,
            cfg(),
            None,
        )
        .unwrap();
        assert_eq!(ids(&g, &out), vec![p.id, t[0].id, t[1].id, t[2].id]);
        assert_eq!(g.episode(&out).unwrap().own_turns(), vec![p.id]);
        assert_eq!(ids(&g, &base), vec![t[0].id, t[1].id, t[2].id], "untouched");
    }

    #[test]
    fn a_user_strategy_can_prune() {
        // The environment claim: a user's own strategy (here, "drop turn `at`") is just
        // an impl returning two spans that skip it — no engine change.
        struct Prune {
            episode: EpisodeId,
            at: usize,
        }
        impl MergeStrategy for Prune {
            fn plan(&self, graph: &Graph) -> Result<Vec<Piece>, MergeError> {
                let len = graph.full_turns(&self.episode).len();
                Ok(vec![
                    Piece::Span {
                        episode: self.episode,
                        start: 0,
                        end: self.at,
                    },
                    Piece::Span {
                        episode: self.episode,
                        start: self.at + 1,
                        end: len,
                    },
                ])
            }
        }
        let (mut g, base, t) = root_with(&["a", "b", "c"]);
        let out = EpisodeId::from_seed(b"pruned");
        g.apply_strategy(
            &Prune {
                episode: base,
                at: 1,
            },
            out,
            cfg(),
            None,
        )
        .unwrap();
        assert_eq!(ids(&g, &out), vec![t[0].id, t[2].id], "b pruned → a c");
    }

    #[test]
    fn a_strategy_errors_on_a_missing_episode() {
        let (g, ..) = root_with(&["a"]);
        let ghost = EpisodeId::from_seed(b"ghost");
        let err = MergeStraightIn {
            base: ghost,
            branch: ghost,
        }
        .plan(&g)
        .unwrap_err();
        assert!(matches!(err, MergeError::EpisodeNotFound(_)));
    }

    #[test]
    fn a_merged_episode_reads_as_its_own_session() {
        let (mut g, base, _t) = root_with(&["a", "b", "c"]);
        let branch = EpisodeId::from_seed(b"branch");
        g.branch_prefix(base, 2, branch); // shares [a,b], forks at c
        let d = turn("d");
        g.insert_content(d.clone());
        g.episode_mut(&branch).unwrap().append(d.occurrence());
        let out = EpisodeId::from_seed(b"merged");
        let origin = Some(Origin {
            episode: base,
            turn: g.full_turns(&base)[0],
            index: Some(0),
            kind: EdgeKind::Merge,
        });
        g.apply_strategy(&MergeStraightIn { base, branch }, out, cfg(), origin)
            .unwrap();
        // One node, one place: the merge hangs under its origin turn — it is
        // NOT a second forest root (that double-rendered it) — and it displays
        // its full derived history (a straight merge owns nothing; the derived
        // sequence IS its content).
        let out_ep = g.episode(&out).unwrap();
        assert!(!out_ep.is_session_root(), "a merge nests under its origin");
        assert!(out_ep.shows_full_history());
        assert!(!g.roots().contains(&crate::graph::Node::Episode(out)));
        let anchor = crate::graph::Node::Turn {
            episode: base,
            turn: g.full_turns(&base)[0],
            index: 0,
        };
        assert!(
            g.children(anchor)
                .contains(&crate::graph::Node::Episode(out)),
            "the merge appears under its origin turn — exactly once"
        );
    }

    #[test]
    fn squash_replaces_a_range_with_a_summary() {
        // [a,b,x,y,z,c], squash positions 2..5 (x,y,z) into p → [a,b,p,c]. Only p is
        // new; a,b,c stay shared; the original is untouched.
        let (mut g, ep, t) = root_with(&["a", "b", "x", "y", "z", "c"]);
        let p = turn("p");
        let out = EpisodeId::from_seed(b"squashed");
        g.apply_strategy(
            &Squash {
                episode: ep,
                start: 2,
                end: 5,
                summary: vec![p.clone()],
            },
            out,
            cfg(),
            None,
        )
        .unwrap();
        assert_eq!(
            ids(&g, &out),
            vec![t[0].id, t[1].id, p.id, t[5].id],
            "a b p c"
        );
        assert_eq!(g.episode(&out).unwrap().own_turns(), vec![p.id]);
        assert_eq!(
            ids(&g, &ep).len(),
            6,
            "the original still has all six turns"
        );
    }

    #[test]
    fn squash_with_an_empty_range_just_inserts_the_summary() {
        let (mut g, ep, t) = root_with(&["a", "b", "c"]);
        let p = turn("p");
        let out = EpisodeId::from_seed(b"inserted");
        g.apply_strategy(
            &Squash {
                episode: ep,
                start: 1,
                end: 1,
                summary: vec![p.clone()],
            },
            out,
            cfg(),
            None,
        )
        .unwrap();
        assert_eq!(
            ids(&g, &out),
            vec![t[0].id, p.id, t[1].id, t[2].id],
            "a p b c"
        );
    }

    #[test]
    fn squash_rejects_an_out_of_range_range() {
        let (g, ep, _t) = root_with(&["a", "b", "c"]);
        let bad = Squash {
            episode: ep,
            start: 2,
            end: 9,
            summary: vec![turn("p")],
        }
        .plan(&g)
        .unwrap_err();
        assert!(matches!(bad, MergeError::Unmergeable(_)));
        let reversed = Squash {
            episode: ep,
            start: 3,
            end: 1,
            summary: vec![turn("p")],
        }
        .plan(&g)
        .unwrap_err();
        assert!(matches!(reversed, MergeError::Unmergeable(_)));
    }

    #[test]
    fn squash_merge_is_merge_straight_in_with_a_new_middle() {
        // THE LITMUS as a strategy: base a→b→c, branch off b of x→y→z. Squash-merging
        // with summary p gives a→b→p→c — and it agrees with the two-step route
        // (merge straight in, THEN squash the branch's span into p). One piece of the
        // difference between merge and squash: the middle.
        let (mut g, base, t) = root_with(&["a", "b", "c"]);
        let branch = EpisodeId::from_seed(b"branch");
        g.branch_prefix(base, 2, branch);
        let (x, y, z) = (turn("x"), turn("y"), turn("z"));
        for tt in [&x, &y, &z] {
            g.insert_content(tt.clone());
            g.episode_mut(&branch).unwrap().append(tt.occurrence());
        }
        let p = turn("p");

        // Route 1: SquashMerge directly.
        let direct = EpisodeId::from_seed(b"squash_merged");
        g.apply_strategy(
            &SquashMerge {
                base,
                branch,
                summary: vec![p.clone()],
            },
            direct,
            cfg(),
            None,
        )
        .unwrap();
        assert_eq!(
            ids(&g, &direct),
            vec![t[0].id, t[1].id, p.id, t[2].id],
            "a b p c"
        );

        // Route 2: merge straight in, then squash the spliced x,y,z (positions 2..5).
        let merged = EpisodeId::from_seed(b"merged");
        g.apply_strategy(&MergeStraightIn { base, branch }, merged, cfg(), None)
            .unwrap();
        let two_step = EpisodeId::from_seed(b"two_step");
        g.apply_strategy(
            &Squash {
                episode: merged,
                start: 2,
                end: 5,
                summary: vec![p.clone()],
            },
            two_step,
            cfg(),
            None,
        )
        .unwrap();
        assert_eq!(
            ids(&g, &direct),
            ids(&g, &two_step),
            "squash-merge == merge-then-squash — the foundation composes consistently"
        );
    }

    #[test]
    fn pick_assembles_selected_spans_from_one_episode() {
        // The base `pick`: keep the flanks around a dropped turn (a prune) — [a,b,c] with
        // b dropped → [a,c], every turn shared by reference.
        let (mut g, ep, t) = root_with(&["a", "b", "c"]);
        let out = EpisodeId::from_seed(b"picked");
        g.apply_strategy(
            &Pick {
                picks: vec![(ep, 0, 1), (ep, 2, 3)],
            },
            out,
            cfg(),
            None,
        )
        .unwrap();
        assert_eq!(ids(&g, &out), vec![t[0].id, t[2].id], "a c — b picked out");
        assert!(
            g.episode(&out).unwrap().own_turns().is_empty(),
            "a pick owns nothing — every turn is a shared span"
        );
        assert_eq!(ids(&g, &ep).len(), 3, "the original is untouched");
    }

    #[test]
    fn pick_gathers_across_multiple_episodes() {
        // The N-way gather: collect the best span from each of two variants into one
        // composed episode — pick reads from any number of sources.
        let (mut g, e1, t1) = root_with(&["a", "b"]);
        let e2 = EpisodeId::from_seed(b"variant");
        let (x, y) = (turn("x"), turn("y"));
        let mut ep2 = Episode::root(e2, cfg());
        for tt in [&x, &y] {
            g.insert_content(tt.clone());
            ep2.append(tt.occurrence());
        }
        g.insert_episode(ep2);

        let out = EpisodeId::from_seed(b"gathered");
        g.apply_strategy(
            &Pick {
                picks: vec![(e1, 0, 1), (e2, 1, 2)],
            },
            out,
            cfg(),
            None,
        )
        .unwrap();
        assert_eq!(
            ids(&g, &out),
            vec![t1[0].id, y.id],
            "a (from e1) + y (from the variant e2)"
        );
    }

    #[test]
    fn pick_errors_on_a_missing_episode() {
        let (g, ..) = root_with(&["a"]);
        let ghost = EpisodeId::from_seed(b"ghost");
        let err = Pick {
            picks: vec![(ghost, 0, 1)],
        }
        .plan(&g)
        .unwrap_err();
        assert!(matches!(err, MergeError::EpisodeNotFound(_)));
    }

    #[test]
    fn re_root_replaces_the_prefix_and_keeps_the_suffix() {
        // Replace the old root [r0,r1] with a fresh [n0,n1], keep the suffix [t2,t3].
        let (mut g, ep, t) = root_with(&["r0", "r1", "t2", "t3"]);
        let (n0, n1) = (turn("n0"), turn("n1"));
        let out = EpisodeId::from_seed(b"rerooted");
        g.apply_strategy(
            &ReRoot {
                episode: ep,
                drop: 2,
                root: vec![n0.clone(), n1.clone()],
            },
            out,
            cfg(),
            None,
        )
        .unwrap();
        assert_eq!(
            ids(&g, &out),
            vec![n0.id, n1.id, t[2].id, t[3].id],
            "new root + shared suffix"
        );
        assert_eq!(
            g.episode(&out).unwrap().own_turns(),
            vec![n0.id, n1.id],
            "only the new root is new data; the suffix is shared"
        );
        assert_eq!(ids(&g, &ep).len(), 4, "the original is untouched");
    }

    #[test]
    fn re_root_with_drop_zero_is_a_prepend() {
        let (mut g, ep, t) = root_with(&["a", "b"]);
        let p = turn("p");
        let out = EpisodeId::from_seed(b"prepended");
        g.apply_strategy(
            &ReRoot {
                episode: ep,
                drop: 0,
                root: vec![p.clone()],
            },
            out,
            cfg(),
            None,
        )
        .unwrap();
        assert_eq!(ids(&g, &out), vec![p.id, t[0].id, t[1].id], "p a b");
    }

    #[test]
    fn re_root_rejects_dropping_more_than_exists() {
        let (g, ep, _t) = root_with(&["a", "b"]);
        let err = ReRoot {
            episode: ep,
            drop: 3,
            root: vec![turn("p")],
        }
        .plan(&g)
        .unwrap_err();
        assert!(matches!(err, MergeError::Unmergeable(_)));
    }

    #[test]
    fn reconstruct_plans_minimal_pieces() {
        // The deterministic PLAN-SHAPE spec for the universal edit primitive.
        // (Semantics — round-trip + never-re-own-known, over arbitrary
        // within/across-session targets — are property-tested below; this
        // test pins the piece STRUCTURE the property can't see. The M3 test
        // review folded the fixed-case round-trip/COW unit tests into the
        // property for exactly that split.)
        //
        // target [a, X, c] over the known [a,b,c]: shares a and c by
        // reference, owns only X — `[Span, New, Span]`, decided by content.
        let (mut g, ep, t) = root_with(&["a", "b", "c"]);
        let x = turn("x");
        let target = vec![t[0].clone(), x.clone(), t[2].clone()];
        let pieces = g.reconstruct(&target);
        assert_eq!(
            pieces,
            vec![
                Piece::Span {
                    episode: ep,
                    start: 0,
                    end: 1
                },
                Piece::New(vec![x.clone()]),
                Piece::Span {
                    episode: ep,
                    start: 2,
                    end: 3
                },
            ]
        );
        let out = EpisodeId::from_seed(b"reconstructed");
        g.compose(out, cfg(), pieces, None);
        assert_eq!(ids(&g, &out), vec![t[0].id, x.id, t[2].id]);
        assert_eq!(g.episode(&out).unwrap().own_turns(), vec![x.id]);

        // And the non-contiguous middle: edit turns 1 and 3 of [a,b,c,d,e] —
        // the unchanged MIDDLE turn c stays shared, the win over a
        // prefix/suffix diff (which would own the whole [1..4) including c).
        let (mut g, _ep, t) = root_with(&["a", "b", "c", "d", "e"]);
        let (bb, dd) = (turn("B"), turn("D"));
        let target = vec![
            t[0].clone(),
            bb.clone(),
            t[2].clone(),
            dd.clone(),
            t[4].clone(),
        ];
        let pieces = g.reconstruct(&target);
        let out = EpisodeId::from_seed(b"noncontig");
        g.compose(out, cfg(), pieces, None);
        assert_eq!(ids(&g, &out), vec![t[0].id, bb.id, t[2].id, dd.id, t[4].id]);
        assert_eq!(
            g.episode(&out).unwrap().own_turns(),
            vec![bb.id, dd.id],
            "only the two edited turns are owned; a, c, e stay shared"
        );
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        /// One step of a generated edit: reuse some existing turn, or author
        /// one from the same tag alphabet — which may therefore *collide*
        /// content-wise with an existing turn, in which case it IS that turn
        /// (content addressing) and reconstruct must share it, not own it.
        #[derive(Debug, Clone)]
        enum Step {
            Existing(prop::sample::Index),
            Authored(Made),
        }

        /// A turn as generated: its tag, and when it was sealed — so two
        /// turns of equal content (one `TurnId`) can be two productions, the
        /// distinction `reconstruct` must keep. Every turn was sealed at one
        /// instant, so sharing by content alone passed this property (review
        /// stop 12, T9 6).
        type Made = (u8, u64);

        fn arb_made(tags: std::ops::Range<u8>) -> impl Strategy<Value = Made> {
            (tags, 0u64..3)
        }

        fn made((tag, secs): Made) -> Turn {
            Turn::seal(
                vec![
                    Message::user(format!("t{tag}")),
                    Message::assistant(format!("t{tag}")),
                ],
                cfg(),
                SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs),
            )
        }

        fn arb_step() -> impl Strategy<Value = Step> {
            prop_oneof![
                any::<prop::sample::Index>().prop_map(Step::Existing),
                // 0..6 collides with session content; 6..9 is genuinely new.
                arb_made(0..9).prop_map(Step::Authored),
            ]
        }

        /// A root episode over `tags`, added to `g` under `seed`.
        fn add_root(g: &mut Graph, seed: &[u8], tags: &[Made]) -> Vec<Turn> {
            let turns: Vec<Turn> = tags.iter().copied().map(made).collect();
            let id = EpisodeId::from_seed(seed);
            let mut ep = Episode::root(id, cfg());
            for t in &turns {
                g.insert_content(t.clone());
                ep.append(t.occurrence());
            }
            g.insert_episode(ep);
            turns
        }

        proptest! {
            // The invariant the whole edit model rests on (design.md §1.2),
            // property-tested: for ANY target sequence assembled from existing
            // turns (within/across sessions, repeated, reordered) and authored
            // ones, compose(reconstruct(target)) == target — and reconstruct
            // owns only turns the graph has never seen (the COW guarantee).
            // A tiny tag alphabet makes duplicate content — shared TurnIds
            // within and across sessions — common, which is exactly the
            // occurrence-vs-content minefield ([[content-addressed-ids]]);
            // and a tiny seal-time range makes equal content with other facts
            // common too — one id, two productions.
            #[test]
            fn compose_of_reconstruct_reproduces_any_target(
                base_tags in prop::collection::vec(arb_made(0..6), 1..6),
                other_tags in prop::collection::vec(arb_made(0..6), 0..4),
                (branch_at, branch_tag) in (any::<prop::sample::Index>(), arb_made(0..9)),
                steps in prop::collection::vec(arb_step(), 0..8),
            ) {
                let mut g = Graph::new();
                let mut pool = add_root(&mut g, b"p1", &base_tags);
                pool.extend(add_root(&mut g, b"p2", &other_tags));
                // A real BRANCH too (prefix Slice + one own turn), so the pool
                // includes turns reached through structural sharing and owner
                // canonicalization — the shape production graphs actually have,
                // not just flat roots.
                let edited = made(branch_tag);
                let forked = g.branch(
                    EpisodeId::from_seed(b"p1"),
                    branch_at.index(base_tags.len()),
                    edited.clone(),
                    EpisodeId::from_seed(b"br"),
                );
                prop_assert!(forked.is_some(), "branch index is in range by construction");
                pool.push(edited);

                let target: Vec<Turn> = steps
                    .iter()
                    .map(|s| match s {
                        Step::Existing(idx) => pool[idx.index(pool.len())].clone(),
                        Step::Authored(m) => made(*m),
                    })
                    .collect();

                let pieces = g.reconstruct(&target);
                // COW: no production the graph already has is owned again —
                // a production is the content AND its facts: equal content
                // under other facts IS owned (it is a new occurrence). This
                // said "no content the graph knows", false of that rule.
                for piece in &pieces {
                    if let Piece::New(turns) = piece {
                        for t in turns {
                            prop_assert!(
                                !pool.contains(t),
                                "reconstruct owned a production the graph already had"
                            );
                        }
                    }
                }
                let out = EpisodeId::from_seed(b"out");
                g.compose(out, cfg(), pieces, None);
                prop_assert_eq!(
                    g.records(&out),
                    Some(target),
                    "compose(reconstruct(target)) must reproduce target exactly, facts included"
                );
            }
        }
    }
}
