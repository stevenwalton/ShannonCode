//! **Session ops** — the graph-surgery half of the App: composing, editing,
//! squashing, picking, cloning, checking out, and persisting episodes. Split
//! from `app.rs` at the P6.0 checkpoint (the tracked monolith debt): `app.rs`
//! keeps the event loop, input dispatch, and channel lifecycles; every
//! operation that *reshapes the graph* lives here. A child module of `app`, so
//! it sees the same private `App` fields — the split is organizational, not an
//! API boundary. Items are `pub(crate)` and glob-re-exported by `app.rs`, so
//! callers and tests are unchanged.

use super::*;

/// **How a composition reaches the screen** — sync versus async, which is part
/// of the rule and invisible in the arguments: a viewport is taken back
/// because you just asked for something, never because something you asked
/// for earlier arrived (the `:continue` landing's rule, made a value).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Landing {
    /// You just asked — a key, a reviewed `$EDITOR`, a `y`: the result is
    /// shown, a viewer that cannot show it handing its pane back.
    Asked,
    /// It arrived later (an unreviewed `:compact`'s summary): the trees are rebuilt and
    /// no viewport moves — you may be reading something else by now.
    Arrived,
}

/// **Where a composed session lands** (`buffers.md` §9, call 1 — Steven,
/// 10-20: "A prompt in flight should stay in flight unless someone `<C-c>`
/// or `:q!`/`:qa!`s a conversation"). [`App::land`] is its door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lands {
    /// **It goes on AS this buffer's session** — `:continue`, a squash of a
    /// session's own turns (`adopt_composed`'s `carries`): rewritten in
    /// place. Nothing else runs there: the continuation or the summary held
    /// the session until now.
    Over(BufferId),
    /// **A session beside its sources** — a clone, a pick, a merge, a
    /// return, an edit's branch, a branch's or a gathered summary, an import
    /// — landing where this buffer (the aim, at the keypress) is shown. In
    /// that buffer while it is blank — no history, nothing running, one pane
    /// at most, so nobody could tell — else in a buffer of its own, shown in
    /// that buffer's window and aimed at. The session it displaced keeps its
    /// buffer — its work running, its unwritten turns kept — as vim's `:e`
    /// keeps the buffer it leaves (`:b#`). They rewrote the aim in place: a
    /// turn streaming there was cut off with "the turn you left was
    /// interrupted", though nothing was left, and a lazy session was lost
    /// (review 2026-10-15, A F5). Reached by keystrokes only — a result that
    /// arrives later replaces (`Over`) or stands apart (`Apart`).
    Beside(BufferId),
    /// **Beside this buffer, opened in this window** — `<CR>` in a tree on a
    /// Claude Code conversation brought in: in the tree's own window, the
    /// tree kept as its alternate, as a session's `<CR>` opens there (V4b
    /// Q1). The window takes it whatever it shows ([`App::enter_session`]);
    /// the buffer it lands beside is as for [`Beside`](Lands::Beside).
    Into(BufferId, crate::wm::PaneId),
    /// **A session beside its sources that ARRIVED** — an unreviewed `S`'s
    /// branch or gathered summary (`[merge] distill_review` off): a buffer
    /// of its own, shown in no pane and not aimed at. Shown where you were,
    /// it would move a pane you may be typing in, because something you
    /// asked for earlier came back — a viewport is taken because you asked,
    /// never because an answer arrived (full review 10-21, A 8). Said, with
    /// how to show it.
    Apart,
}

/// What a squash summary will replace once approved — resolved when `s`/`S` is
/// pressed, consumed when the reviewed summary composes. Two shapes, one tail:
/// a branch's divergent turns at its fork ([`SquashMerge`]) or a `<Space>`-marked
/// turn range in place ([`Squash`] — user-driven compaction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SquashTarget {
    /// Squash-merge the branch into `base` at the fork: `A→B→P→C`.
    Branch {
        base: EpisodeId,
        branch: EpisodeId,
        fork_turn: TurnId,
        /// `fork_turn`'s occurrence index in `base`.
        fork_index: usize,
        config: TurnConfig,
    },
    /// Squash `episode`'s turns `[start, end)` in place: `A→B→P→C` from one line.
    Range {
        episode: EpisodeId,
        start: usize,
        end: usize,
        fork_turn: TurnId,
        config: TurnConfig,
    },
    /// Distill an arbitrary mark set into a NEW session containing only the
    /// summary — the vision's "distill based on selected nodes." Reached when
    /// marks don't read as one contiguous strip of a single timeline
    /// (cross-session, gaps, episodes mixed with turns). `episode`/`turn` are
    /// the first marked node, a presentation origin only; the real record is
    /// the summary's `distilled_from`.
    Gather {
        episode: EpisodeId,
        turn: TurnId,
        /// `turn`'s occurrence index in `episode`.
        index: usize,
        config: TurnConfig,
    },
}

impl SquashTarget {
    /// **The session whose turns the summary replaces** — the one a turn
    /// added or cut makes its draft stale over ([`stale_target`]): a range's,
    /// or the branch folded. A gather names its turns by id and composes a
    /// session of its own, so none. A review holds this session as well as
    /// its own (`App::review_holding`; review stop 8, A 4: a prompt into the
    /// branch, or a turn deleted there, waited for the review's `a` to throw
    /// the draft away).
    pub(crate) fn replaces(&self) -> Option<EpisodeId> {
        match self {
            SquashTarget::Branch { branch, .. } => Some(*branch),
            SquashTarget::Range { episode, .. } => Some(*episode),
            SquashTarget::Gather { .. } => None,
        }
    }
}

/// A resolved squash, as [`App::squash_plan`] hands it to `s` and `S`.
pub(crate) struct SquashPlan {
    /// The graph the target was resolved in, with its sources' bodies loaded —
    /// what the summary later composes into.
    pub(crate) graph: Graph,
    pub(crate) target: SquashTarget,
    /// The turns the summary stands in for (its `distilled_from`).
    pub(crate) sources: Vec<TurnId>,
    /// Their messages: the `$EDITOR` seed, or the generation request.
    pub(crate) msgs: Vec<Message>,
}

/// What `e` edits, as [`App::edit_plan`] resolves it: turns `[start, end)` of
/// `episode`, or one message of one turn, and the seed `$EDITOR` opens on.
pub(crate) struct EditPlan {
    /// The graph the slice was resolved in, with the session's bodies loaded —
    /// what a message edit splices back into.
    pub(crate) graph: Graph,
    pub(crate) episode: EpisodeId,
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// A message row: that message of that turn, by occurrence index.
    pub(crate) message_scope: Option<(TurnId, usize)>,
    pub(crate) seed: String,
}

/// The summary-only composition behind [`SquashTarget::Gather`]: the plan is a
/// single `New` piece, so the composed session contains nothing but the summary
/// turns — whose `distilled_from` provenance points back at everything marked.
/// Deliberately implemented HERE, outside `shannon-core`: the first out-of-crate
/// `MergeStrategy`, which turns the environment claim ("users can write their
/// own strategies against the public surface") into a compile-checked fact.
pub(crate) struct GatherSummary {
    summary: Vec<Turn>,
}

impl MergeStrategy for GatherSummary {
    fn plan(&self, _graph: &Graph) -> Result<Vec<Piece>, MergeError> {
        Ok(vec![Piece::New(self.summary.clone())])
    }
}

/// The **return** composition (P6.3): the parent's whole history plus ONE new
/// turn carrying a finished line's result — `[Span(parent, 0..len), New(result)]`.
/// This is what a child session collapsing back into its caller looks like:
/// briefing in, final answer out, the working turns staying behind in the
/// child (reachable through the result's `distilled_from`). Nothing about it
/// is child-specific — the pattern doesn't care who the participants were, so
/// a branch returns by the exact same plan.
pub(crate) struct ResultInject {
    parent: EpisodeId,
    result: Turn,
}

impl MergeStrategy for ResultInject {
    fn plan(&self, graph: &Graph) -> Result<Vec<Piece>, MergeError> {
        if graph.episode(&self.parent).is_none() {
            return Err(MergeError::EpisodeNotFound(self.parent.to_hex()));
        }
        // The parent's length is read HERE, not carried in, so the plan can't
        // go stale between building the strategy and composing.
        let len = graph.full_turns(&self.parent).len();
        Ok(vec![
            Piece::Span {
                episode: self.parent,
                start: 0,
                end: len,
            },
            Piece::New(vec![self.result.clone()]),
        ])
    }
}

/// Where a composed session hangs in the tree: the turn it presents as forking
/// from, and that turn's occurrence index.
///
/// The three always travel together — they are exactly [`Origin`] minus the
/// edge kind — and passing them separately is how a caller ends up handing over
/// a `fork_index` that belongs to a different `fork_turn`
/// (`[[feedback_content_addressed_ids]]`: an index is what pins an occurrence).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Anchor {
    pub(crate) base: EpisodeId,
    pub(crate) turn: TurnId,
    /// `turn`'s occurrence index within `base`.
    pub(crate) index: usize,
}

/// Resolve non-empty `<Space>` marks into one contiguous turn range of a single
/// episode: `(episode, start, end)`. One marked episode row means **exactly the
/// turns the tree displays under it**. Anything else — marks across episodes,
/// an episode mixed with turns, or a gap — is refused, with a reason the caller
/// must surface: silently widening to `min..max` would squash turns the user
/// never marked, the data-loss shape we don't tolerate.
pub(crate) fn marked_turn_range(
    marked: &[Node],
    graph: &Graph,
) -> Result<(EpisodeId, usize, usize), String> {
    // **Expand a lone episode row through the function the TREE uses**, so
    // mark-the-row and mark-each-shown-turn cannot drift. (The dogfooding bug:
    // the tree showed a branch's 2 divergent turns but the row was taken as its
    // full 4-turn derived history — "distilling 4 turns" while the screen said
    // 2.) A root owns everything, so its row still means the whole
    // conversation; a branch row means its divergent tail.
    //
    // This said the same thing and called `Graph::children`, which is the same
    // filter **minus the `shows_full` term** — so for a clone or a merge, which
    // show a full history they do not own, the row expanded to nothing and
    // refused with "owns no turns" about a row with turns drawn under it
    // (Reviewer D). Ownership is a concern for `d`, which destroys; squash and
    // merge compose a NEW session and leave the originals alone, so the row can
    // safely mean what it shows. Steven's call, 2026-08-25.
    let expanded: Vec<Node>;
    let marked = if let [Node::Episode(id)] = marked {
        expanded = crate::graph_view::episode_children(graph, *id)
            .into_iter()
            .map(|drawn| drawn.node)
            .collect();
        if expanded.is_empty() {
            return Err(format!(
                "squash: {} has no turns yet — send one first",
                crate::view::session_tag_in(graph, *id)
            ));
        }
        expanded.as_slice()
    } else {
        marked
    };
    let mut episode: Option<EpisodeId> = None;
    let mut indices: Vec<usize> = Vec::with_capacity(marked.len());
    for node in marked {
        let (ep, index) = match *node {
            Node::Turn { episode, index, .. } => (episode, index),
            Node::Message {
                episode,
                turn_index,
                ..
            } => (episode, turn_index),
            Node::Episode(_) => {
                return Err(
                    "squash: mark either one whole session or a run of turns, not both".into(),
                );
            }
        };
        if *episode.get_or_insert(ep) != ep {
            return Err(
                "squash: marks span sessions — squash one session's turns (p picks across)".into(),
            );
        }
        indices.push(index);
    }
    indices.sort_unstable();
    indices.dedup(); // both messages of a turn marked = that one turn
    // Defensive: the callers guard emptiness before reaching here (the doc
    // comment's "non-empty marks" precondition), but this is `pub(crate)` and
    // the `[first]`/`[len-1]` indexing below would panic on an empty set — a
    // future caller shouldn't be able to turn a missed guard into a crash.
    if indices.is_empty() {
        return Err("squash: nothing marked".into());
    }
    let (first, last) = (indices[0], indices[indices.len() - 1]);
    if last - first + 1 != indices.len() {
        return Err("squash: marked turns have gaps — mark a contiguous run".into());
    }
    let episode = episode.ok_or("squash: nothing marked")?;
    Ok((episode, first, last + 1))
}

/// Resolve an arbitrary mark set into a **gather**: the marked turns, in the
/// order the tree displays them (preorder = top-to-bottom on screen), destined
/// to distill into a new summary-only session. Episode rows expand to exactly
/// their displayed turns (same `Graph::children` as the tree); a turn marked
/// twice (via both its messages) counts once, but a *shared* turn marked under
/// two sessions is two occurrences — marks are occurrences, never content-id
/// deduped (`[[content-addressed-ids]]`). Errs only when nothing marked resolves
/// to a turn (e.g. a lone merge/clone row, which owns none).
pub(crate) fn gather_target(
    marked: &[Node],
    graph: &Graph,
) -> Result<(SquashTarget, Vec<TurnId>, Vec<Message>), String> {
    let mut pairs: Vec<(EpisodeId, usize)> = Vec::new();
    let push_unique = |pairs: &mut Vec<(EpisodeId, usize)>, pair| {
        if !pairs.contains(&pair) {
            pairs.push(pair);
        }
    };
    for node in marked {
        match *node {
            Node::Episode(id) => {
                for child in graph.children(Node::Episode(id)) {
                    if let Node::Turn { episode, index, .. } = child {
                        push_unique(&mut pairs, (episode, index));
                    }
                }
            }
            Node::Turn { episode, index, .. } => push_unique(&mut pairs, (episode, index)),
            Node::Message {
                episode,
                turn_index,
                ..
            } => push_unique(&mut pairs, (episode, turn_index)),
        }
    }
    // Resolve pairs to turn ids together, so a stale index (shouldn't happen —
    // the view snapshot and this graph are the same) drops its pair instead of
    // desynchronizing sources from their origins.
    let resolved: Vec<(EpisodeId, usize, TurnId)> = pairs
        .iter()
        .filter_map(|&(ep, i)| graph.full_turns(&ep).get(i).map(|t| (ep, i, *t)))
        .collect();
    let Some(&(origin_episode, origin_index, origin_turn)) = resolved.first() else {
        return Err("squash: the marked rows own no turns to distill — mark turns".into());
    };
    let sources: Vec<TurnId> = resolved.iter().map(|&(.., t)| t).collect();
    let msgs = collect_messages(graph, &sources)
        .ok_or("squash: part of the marked history is not in the store")?;
    if msgs.is_empty() {
        return Err("squash: the marked turns have no messages to distill".into());
    }
    let config = graph
        .episode(&origin_episode)
        .ok_or("squash: the first marked session is not in the graph")?
        .config
        .clone();
    Ok((
        SquashTarget::Gather {
            episode: origin_episode,
            turn: origin_turn,
            index: origin_index,
            config,
        },
        sources,
        msgs,
    ))
}

/// The full message list behind `sources`, in order — the `$EDITOR` seed and
/// the distillation request body. `None` if any body is not loaded: a seed
/// or a summary missing a turn silently would misstate what it stands for.
pub(crate) fn collect_messages(graph: &Graph, sources: &[TurnId]) -> Option<Vec<Message>> {
    let mut out = Vec::new();
    for id in sources {
        out.extend(graph.messages(id)?.iter().cloned());
    }
    Some(out)
}

/// Whether `target` still names, in `graph`, the turns `sources` were read
/// from — the reason when it does not. A range must still hold exactly those
/// turns at those positions; a branch must still end in them. A gather names
/// its turns by id, and composes a session of its own, so it cannot go stale.
fn stale_target(graph: &Graph, target: &SquashTarget, sources: &[TurnId]) -> Option<String> {
    // Gone is said as gone: the merge said it in its own words ("episode not
    // found: <64 hex>"), or an empty range read as "changed" (review stop 8,
    // D 3).
    if let Some(replaced) = target.replaces()
        && graph.episode(&replaced).is_none()
    {
        return Some("the session it replaces is gone".to_string());
    }
    let still = match target {
        SquashTarget::Range {
            episode,
            start,
            end,
            ..
        } => graph.full_turns(episode).get(*start..*end) == Some(sources),
        SquashTarget::Branch { branch, .. } => graph.full_turns(branch).ends_with(sources),
        SquashTarget::Gather { .. } => true,
    };
    // "Since it was asked for": a reviewed draft waits as long as you leave
    // it, so "while it was being written" was false of a change made after
    // (review stop 8, A 4).
    (!still).then(|| "the session changed since the summary was asked for".to_string())
}

/// **`:compact`'s words**: its flags, then the summary's instructions —
/// `(keep, instructions)`. `--keep N` (or `--keep=N`) keeps the last N turns
/// as they were for this one compaction (Steven, 10-16: "I like your idea
/// about `--keep`"), the one-off `[merge] compact_keep`, where `:compact N`
/// once was. In the grammar every flag-taking command shares
/// ([`parse_flags`](crate::flags::parse_flags)): flags lead, `--` ends them,
/// and a flag it does not know is said, not read as words for the model. It
/// had its own reading of that grammar, and the two already disagreed — `-k`
/// a flag everywhere else and words here (review stop 2, C 5).
pub(crate) fn compact_args(args: &str) -> Result<(Option<usize>, &str), String> {
    let keep = crate::flags::FlagSpec {
        long: "keep",
        short: None,
        takes_value: true,
    };
    let (flags, instructions) = crate::flags::parse_flags(args, &[keep]).map_err(|why| {
        format!(
            "`:compact`: {why} — `--keep N` keeps the last N turns as they were; the rest is \
             instructions for the summary (`--` before ones that begin with `-`)"
        )
    })?;
    let keep = flags
        .last()
        .and_then(|flag| flag.value.as_deref())
        .map(|n| match n.parse::<usize>() {
            Ok(n) => Ok(n),
            Err(_) if n.is_empty() => {
                Err("`--keep=` takes how many last turns to keep — `--keep=2`".to_string())
            }
            Err(_) => Err(format!(
                "`--keep` takes how many last turns to keep, a whole number — not `{n}`"
            )),
        })
        .transpose()?;
    Ok((keep, instructions.trim()))
}

/// Rebuild `turn`'s message list with message `index` replaced by the parsed
/// edit — the message-level `e`. The edit may parse to zero messages (delete
/// this message) or several (split it); the rest of the turn is untouched.
/// Returns the rebuilt turn as editable JSON, ready for the turn-slice path.
pub(crate) fn spliced_message_edit(
    graph: &Graph,
    turn: TurnId,
    index: usize,
    edited: &str,
) -> Result<String, String> {
    let parsed =
        from_editable(edited).map_err(|e| format!("edit cancelled: could not parse ({e})"))?;
    let content = graph
        .messages(&turn)
        .ok_or("edit: that turn is not in the graph")?;
    if index >= content.len() {
        return Err("edit: that message is not in the turn".into());
    }
    let mut messages = content.to_vec();
    messages.splice(index..index + 1, parsed);
    Ok(to_editable(&messages))
}

/// **Where a turn goes when `start..end` is squashed into a summary of
/// `summary` turns** — a mark's, carried ([`App::marks_carried`]): before
/// the squash, where it was; after it, moved by the summary's length less
/// the turns folded (a multi-turn summary seals as several: counted as one,
/// every later mark landed a turn early — the close's verifier, V 1);
/// folded, `None`.
fn squashed(start: usize, end: usize, summary: usize) -> impl Fn(usize) -> Option<usize> {
    edited_turns(start, start, end, summary)
}

/// **Where a turn goes in an edit's branch** — the turns before `changed`
/// (the untouched prefix and the slice's unchanged lead) where they were;
/// the untouched suffix from `end` moved by how the slice's length changed
/// (`slice` turns where there were `end - start`); the changed ones `None`.
/// Checked: a turn read from a header can stand anywhere (review stop 14,
/// B F2; the close's verifier, V 4).
fn edited_turns(
    start: usize,
    changed: usize,
    end: usize,
    slice: usize,
) -> impl Fn(usize) -> Option<usize> {
    move |turn| match turn {
        turn if turn < changed => Some(turn),
        turn if turn >= end => (turn - end).checked_add(start)?.checked_add(slice),
        _ => None,
    }
}

impl App {
    /// Persist an episode's full segment **structure**: the header, then each
    /// segment as an append record (`Slice` → [`Store::append_slice`], `Own` →
    /// [`Store::append_turn`]) in order — `own` supplies the turns the `Own`
    /// segments name, oldest first. The single write-side twin of
    /// [`Graph::splice`](shannon_core::Graph): the same `segments` drive the
    /// in-memory episode and its persistence, so there is no second hand-rolled
    /// encoder to drift.
    ///
    /// **Fail-stop with rollback**: the first write error deletes the half-written
    /// episode and returns the error, so a partial structure is never left on disk
    /// to be read back as if whole — which (append-only) could otherwise duplicate a
    /// slice on the next retry, or resolve to a silently truncated history. Used for
    /// the first persist of every episode carrying shared structure: clone edits,
    /// branch commits, and the first-turn replay in [`persist_committed_turn`].
    ///
    /// [`persist_committed_turn`]: Self::persist_committed_turn
    pub(crate) fn persist_episode(&mut self, episode: &Episode, own: &[Turn]) -> StoreResult<()> {
        self.store.put_episode(episode)?;
        let mut own_iter = own.iter();
        let mut result = Ok(());
        'segments: for seg in episode.segments() {
            match seg {
                Segment::Slice {
                    episode: src,
                    start,
                    end,
                } => {
                    if let Err(e) = self.store.append_slice(&episode.id, src, *start, *end) {
                        result = Err(e);
                        break 'segments;
                    }
                }
                Segment::Own(occurrences) => {
                    for _ in occurrences {
                        let Some(turn) = own_iter.next() else { break };
                        if let Err(e) = self.store.append_turn(&episode.id, turn) {
                            result = Err(e);
                            break 'segments;
                        }
                    }
                }
            }
        }
        if result.is_err() {
            // Roll back the partial structure so a retry starts clean (append-only:
            // never leave an incomplete episode to be resolved).
            //
            // **A failed rollback is worse than the write that prompted it** — a
            // half-written, structurally incomplete episode is left on disk, and
            // the caller's error handling only knows about the original failure.
            // `error` rather than `warn`: nothing the user did caused it and the
            // store is now in a state we did not intend.
            if let Err(e) = self.store.delete_episode(&episode.id) {
                tracing::error!(
                    episode = %episode.id.to_hex(),
                    error = %e,
                    "rollback failed; an incomplete episode is left on disk",
                );
            }
        }
        result
    }

    /// `:clone` — **fork a session**. From `:graph` (the `c` key) it forks at the
    /// **selected node**: a turn/message clones the history *up to and including*
    /// that turn, an episode clones the whole thing at its head. The bare
    /// `:clone` *command* forks whatever the graph has selected, else the active
    /// session at its head; `:clone <hash>` forks one by id at its head.
    ///
    /// **With the tree focused, what it picks** — its marks, else the cursor
    /// row, a turn row meaning its session ([`one_graph_target`], which
    /// refuses several) — as `d` and bare `:delete` read it. It read the
    /// cursor alone, so with marks set `:clone` cloned a row nobody marked,
    /// where `c` (now retired to this command) had honoured them.
    ///
    /// [`one_graph_target`]: Self::one_graph_target
    pub(crate) fn clone_session(&mut self) {
        // A selection always wins — you are pointing at something. A viewer
        // is a pane beside the command line, not a takeover, so this is the
        // ordinary path when `:clone` is typed with the tree open.
        if let Some(id) = self.bare_session_target(":clone") {
            self.clone_episode(id);
        }
    }

    /// Bare `:merge` — merge the branch you are pointing at into its base.
    ///
    /// Resolved exactly like [`clone_session`](Self::clone_session), and for
    /// the same reason: a `:graph` selection always wins — the marks, else the
    /// cursor — because a viewer is a pane beside the command line rather than
    /// a takeover, so typing `:merge` with the tree open is the ordinary path,
    /// not a corner case. Falling back to the prompt's own session is what
    /// makes the command usable with no viewer open at all.
    pub(crate) fn merge_session(&mut self) {
        if let Some(target) = self.bare_session_target(":merge") {
            self.merge_branch_into_base(target);
        }
    }

    /// `:return [session]` — **return a session's result into its parent**
    /// (P6.3; it was `:graph`'s `r` key, which is now rename). Bare: with the
    /// tree focused, what it picks, as `:clone` reads it; else the session the
    /// prompt is aimed at — the child you are reading. With a token, that
    /// session ([`session_named`](Self::session_named)).
    pub(crate) fn return_command(&mut self, args: &str) {
        let token = args.trim();
        let target = if token.is_empty() {
            match self.bare_session_target(":return") {
                Some(id) => id,
                None => return,
            }
        } else {
            match self.session_named(token) {
                Ok(id) => id,
                Err(why) => return self.refuse(format!("`:return`: {why}")),
            }
        };
        self.return_result(target);
    }

    /// `:merge <hash>` — merge the session named by a (short) id into its base.
    ///
    /// The refusal is the point of the separate method: an id that matches
    /// nothing must say so rather than merging whatever happens to be selected
    /// ([[feedback_visible_refusals]]).
    pub(crate) fn merge_by_hash(&mut self, hash: &str) {
        match self.session_named(hash) {
            Ok(id) => self.merge_branch_into_base(id),
            Err(why) => self.refuse(format!("`:merge`: {why}")),
        }
    }

    /// `:clone` — **copy `target` by reference into a new session** that shares its
    /// *entire* history (one `Slice`, nothing copied) and switch into it. A clone
    /// starts identical to the original but is its own session: editing a historical
    /// turn on the clone is copy-on-write — that turn becomes the clone's own, the
    /// turns around it (prefix **and** suffix) stay shared, and the original is
    /// untouched. That's the generalization of a branch (which keeps only the
    /// prefix and moves forward); see [`EdgeKind::Clone`] and design.md §1.2. Shown
    /// as its own forest root, with the shared turns marked (italic) in the viewer.
    ///
    /// Lazy, like a checkout — the clone hits disk (and moves `HEAD`) only once it
    /// diverges or continues (an untouched clone is byte-identical to the original,
    /// so there's nothing to persist). Requires `target` to be **persisted** so its
    /// turns resolve on reload; a no-op on a session with no committed turns yet.
    pub(crate) fn clone_episode(&mut self, target: EpisodeId) {
        let (clone_ep, shared, said) = match self.clone_of(target) {
            Ok(clone) => clone,
            Err(why) => return self.refuse(format!("clone: {why}")),
        };
        let clone_id = clone_ep.id;
        // Lazy: minted just now, so absent; persists on first continue or
        // `:write`. Beside the aim: what runs there runs on (`Lands`).
        let from = self.prompt_target();
        let shares = shared.len();
        let Some(landed) = self.land(
            Lands::Beside(from),
            Landing::Asked,
            clone_ep,
            shared,
            OnDisk::Absent,
        ) else {
            return;
        };
        if let Some(clone) = self.session_of_mut(landed) {
            clone.made_as_a_look();
        }
        // Where you were reading it (S21-Q4).
        self.carry_into_shown(target, landed, &|turn| (turn < shares).then_some(turn));
        info!(
            from = %target.to_hex(),
            clone = %clone_id.to_hex(),
            "cloned the session (full share)"
        );
        // Said, as every composing verb and `:fork` say where you are: it
        // landed and took the aim in silence (full review 10-21, D 5).
        let what = format!("cloned {}", self.session_tag(target));
        self.landed_notice(from, landed, &what, "session", "the original kept");
        if let Some(said) = said {
            self.notify(said);
        }
    }

    /// **A clone of `target`, not yet anywhere** — the episode sharing its
    /// whole history by reference, and that history's turns (its working
    /// view) — or why not. What `:clone` adopts into the session in front and
    /// `:fork` opens beside it; the checks are the clone's, not the caller's.
    /// And what it says of the marks it does not carry, for the caller to
    /// say once it has landed ([`Carried::said`](super::marks::Carried::said)).
    #[allow(clippy::type_complexity)]
    pub(crate) fn clone_of(
        &mut self,
        target: EpisodeId,
    ) -> Result<(Episode, Vec<Turn>, Option<String>), String> {
        // Every episode in the snapshot is persisted *except* an unpersisted active
        // session (the one live overlay). The clone's source must be on disk.
        // **Ask the store**, which is the question — the clone's source has to
        // be on disk, because the clone references it by `Slice` and a
        // reference to an episode that was never written is unresolvable on
        // reload. This used to compare `target` against the FOREGROUND and read
        // that buffer's persist counter, a proxy that is wrong twice over: it
        // asks about `current` while `target` comes from the aim, and it misses
        // a lazily-composed session (a squash, a checkout) that owns turns but
        // is not yet written. `checkout_turn` and `commit_slice_edit` both ask
        // the store; this was the third site with the narrower signature
        // ([[feedback_rules_rot_by_retyping]]). And only ABSENCE is "not on
        // disk yet": a store that could not answer says why, rather than
        // telling the user to send a turn in a session that is already saved.
        match self.store.episode_len(&target) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(
                    "that session isn't on disk yet — send a turn in it, or `:write` it".into(),
                );
            }
            Err(e) => return Err(format!("could not read that session: {e}")),
        }
        let graph = self.graph_reading(&[target])?;
        let config = graph
            .episode(&target)
            .map(|e| e.config.clone())
            .ok_or("no such session")?;
        let full_ids = graph.full_turns(&target);
        let &first = full_ids
            .first()
            .ok_or("that session has no turns to clone")?;
        // The working view is the whole shared transcript (rendered from the
        // source's turns); the clone owns none of it yet.
        let shared = graph
            .records(&target)
            .ok_or("part of that session's history is not in the store")?;

        let clone_id = fresh_episode_id();
        // Share the source's entire history by reference (`project`, never copied) —
        // referencing the true data OWNERS, so cloning a clone/branch points straight
        // at the real data, not through the intermediate (keeping the graph shallow
        // and the intermediate deletable). `first` is the origin's attach turn (the
        // session it clones); origin.kind == Clone makes it read as its own session.
        let mut clone_ep = Episode::from_segments(
            clone_id,
            config,
            graph.project(&target, 0, full_ids.len()),
            Some(Origin {
                episode: target,
                turn: first,
                index: Some(0),
                kind: EdgeKind::Clone,
            }),
        );
        // **And its marks** — `:clone`, `:fork`, `:vfork` — on the turns it
        // shares, as its own from here (S16-Q2; review stop 14, A 5: a fork
        // started with none). Not one on a turn still running: the clone
        // shares what was stored.
        let shares = full_ids.len();
        let carried = self.marks_carried(target, &|turn| (turn < shares).then_some(turn));
        let said = carried.said("the clone", Some("their turns are not in it"));
        carried.onto(&mut clone_ep);
        Ok((clone_ep, shared, said))
    }

    /// `:fork` / `:vfork` — **a conversation beside this one** (Steven,
    /// 10-14: "split the pane and branch the conversation — the equivalent of
    /// `/btw`, but extendable"). The session in the pane you are in — or the
    /// one the prompt is aimed at, from the prompt or a viewer — is cloned
    /// ([`clone_of`](Self::clone_of)) into a NEW buffer, shown in a new split
    /// beside it, focused, with the prompt aimed there: ask the side question,
    /// keep going as long as you like, and the original never sees it. `:q`
    /// leaves the fork in `:graph`; `:return` brings its result back (a fork
    /// is a clone, not a branch, so `:merge` refuses it).
    ///
    /// A source not on disk yet is written first, through `:w`'s door — a
    /// clone references what it shares, so it must be written, and it is the
    /// session you are in. One with no turns has nothing to fork.
    pub(crate) fn fork_command(&mut self, args: &str, dir: SplitDir) {
        let verb = match dir {
            SplitDir::Horizontal => ":fork",
            SplitDir::Vertical => ":vfork",
        };
        if !args.trim().is_empty() {
            return self.refuse(format!(
                "`{verb}` forks the session you are in — it takes nothing"
            ));
        }
        let focused = self.workspace.focused();
        let Some(pane) = (self.kind_of(focused) == Some(PaneKind::Output))
            .then_some(focused)
            .or_else(|| self.aim_pane())
        else {
            return self.refuse(format!("`{verb}`: no pane shows a session to fork"));
        };
        let Some((source_buffer, source)) =
            (self.session_for_pane(pane)).map(|b| (b.id, b.conversation.episode().id))
        else {
            return self.refuse(format!("`{verb}`: no pane shows a session to fork"));
        };
        if !self.written_first(source_buffer, verb) {
            return;
        }
        let (fork, shared, said) = match self.clone_of(source) {
            Ok(clone) => clone,
            Err(why) => return self.refuse(format!("`{verb}`: {why}")),
        };
        let fork_id = fork.id;
        // Before the split, so a refusal leaves no pane behind.
        let recall = match self.unlock_turns(&shared) {
            Ok(recall) => recall,
            Err(why) => return self.refuse(format!("`{verb}`: not forked — {why}")),
        };
        let side = self.split_side(dir);
        // Born on the session split from, as `:sp` is: entering the fork
        // keeps it as the new window's alternate (`<C-^>` back to it).
        let born = super::Window::on(source_buffer);
        let at = super::Born::Split {
            target: pane,
            dir,
            side,
        };
        let tab = self.tabs.current();
        let Some(beside) = self.new_window(born, at) else {
            return self.refuse(format!("`{verb}`: that pane can't be split"));
        };
        // Where the window split from is, as `:split` copies it — kept as
        // the alternate once the fork is entered, and where the fork starts
        // (S21-Q4: "Carry over whole place by position").
        let here = self.place_in(pane, source_buffer);
        self.put_place(beside, here.unwrap_or_else(|| self.entry_of(source_buffer)));
        // Lazy, like any clone: written on its first turn or `:write`. The
        // split focused the new pane, as vim's `:split` does; entering it
        // aims the prompt there.
        let shares = shared.len();
        let buffer = self.buffer_holding(fork, shared, OnDisk::Absent, recall, Landing::Asked);
        if let Some(fork) = self.session_of_mut(buffer) {
            fork.made_as_a_look();
        }
        self.enter_session(beside, buffer);
        self.carry_into(beside, source_buffer, buffer, &|turn| {
            (turn < shares).then_some(turn)
        });
        self.refresh_graph_views();
        info!(from = %source.to_hex(), fork = %fork_id.to_hex(), "forked beside the session");
        // `:tab fork` opens it in a tab of its own: not "beside it".
        let place = match self.tabs.current() == tab {
            true => "beside it",
            false => "in a new tab",
        };
        self.notify(format!(
            "forked {} → {} {place} — the original goes on in its own pane",
            self.session_tag(source),
            fork_id.short_hex(),
        ));
        if let Some(said) = said {
            self.notify(said);
        }
    }

    /// **`buffer`'s session, on disk before a clone references it** —
    /// `:fork`'s, since its clone is built by `clone_of`, which needs its
    /// source written, and the source is the session you are in. Written
    /// through `:w`'s own door, every time (a write with nothing pending
    /// costs nothing, and a session registered but missing a later turn is
    /// sliced past what is on disk otherwise); `false`, with the reason on
    /// screen, when there are no turns to write or the write failed. A
    /// composition's sources are `adopt_composed`'s to write
    /// (`write_what_it_shares`); `:clone` refuses an unwritten source —
    /// docketed for Steven to make one rule.
    pub(crate) fn written_first(&mut self, buffer: BufferId, verb: &str) -> bool {
        if self.session_of(buffer).is_none() {
            self.refuse(format!("`{verb}`: that session is no longer open"));
            return false;
        }
        match self.persist_saying(buffer, None) {
            Saved::Empty => {
                self.refuse(format!("`{verb}`: the session has no turns yet"));
                false
            }
            // Said already, on the notice line.
            Saved::Refused | Saved::Gone => false,
            Saved::UpToDate | Saved::Wrote(_) => true,
        }
    }

    /// Resolve a selected episode to a mergeable **branch target**: its base, the
    /// fork turn, and the config the composed line runs under (the base's). `None`
    /// (with a hint) unless the episode is a [`Branch`](EdgeKind::Branch) — merge and
    /// squash both attach a branch's divergence to its base. Reads the passed
    /// snapshot so the caller keeps ownership for the mutation.
    pub(crate) fn merge_target(
        &self,
        graph: &Graph,
        branch: EpisodeId,
    ) -> Result<(EpisodeId, TurnId, usize, TurnConfig), String> {
        let ep = graph
            .episode(&branch)
            .ok_or("that session is not in the graph")?;
        let Some(origin) = ep.origin.filter(|o| o.kind == EdgeKind::Branch) else {
            return Err(format!(
                "{} is not a branch — merge/squash fold a branch into its base",
                self.session_tag(branch)
            ));
        };
        let base = origin.episode;
        // The attach turn's occurrence index in base — stored on new origins;
        // legacy records fall back to the id's first occurrence.
        let fork_index = origin.index.unwrap_or_else(|| {
            graph
                .full_turns(&base)
                .iter()
                .position(|t| *t == origin.turn)
                .unwrap_or(0)
        });
        let config = graph
            .episode(&base)
            .map(|e| e.config.clone())
            .unwrap_or_else(|| ep.config.clone());
        Ok((base, origin.turn, fork_index, config))
    }

    /// Run a composition `strategy` on `graph` and switch into the result as a new
    /// session — written at once when it owns new content (a squash's summary,
    /// [`write_if_it_owns`](Self::write_if_it_owns)), **lazy** when it only
    /// shares (a straight merge: persists on first continue, like a clone) — the shared tail
    /// of `m` (merge) and `s` (squash). Takes the snapshot by value (one build, no
    /// second `current_graph`). The result reads as its own [`EdgeKind::Merge`]
    /// session, hung off `(base, fork_turn)` for presentation; originals untouched.
    /// Returns the new id, or `None` (logged) if the strategy rejects the inputs.
    ///
    /// `buffer` is the buffer the result lands beside. Named rather than
    /// assumed to be the foreground: a composition can resolve long after it
    /// started (`S` streams a summary, `:continue` streams a continuation),
    /// and by then the session in front may be a different one — writing into
    /// `self.buffer_mut()` would then overwrite a session the user merely
    /// switched to (`[[feedback_relocating_state_fans_out]]`).
    ///
    /// `carries` is the session the result **replaces** as the work goes on —
    /// `:continue`, a squash of a session's own turns — whose name and
    /// description move to it, as a rebase moves a branch name
    /// ([`carry_label`](Self::carry_label)); `None` for a result that stands
    /// beside its sources (a pick, a merge, a return, a branch's squash-merge),
    /// which a name does not follow. **Its marks go on too** (the clone
    /// principle, S16-Q2; review stop 14, A 5: `:continue` dropped every
    /// mark, unsaid), each to the turn the map gives — `None` for a turn the
    /// result does not keep as it was, said ([`marks_carried`](Self::marks_carried)).
    ///
    /// **Where it lands follows from `carries` and `landing`** ([`Lands`]): a
    /// result that replaces a session goes on in THAT session's buffer, in
    /// place, whatever `buffer` says; one that stands beside its sources
    /// lands beside `buffer` when you just asked, and apart — in no pane —
    /// when it arrived ([`land`](Self::land)). Returns the new id and the
    /// buffer it landed in.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn adopt_composed(
        &mut self,
        buffer: BufferId,
        mut graph: Graph,
        strategy: &dyn shannon_core::MergeStrategy,
        at: Anchor,
        config: TurnConfig,
        carries: Option<(EpisodeId, super::marks::TurnMap<'_>)>,
        landing: Landing,
    ) -> Option<(EpisodeId, BufferId)> {
        let new_id = fresh_episode_id();
        let origin = Some(Origin {
            episode: at.base,
            turn: at.turn,
            index: Some(at.index),
            kind: EdgeKind::Merge,
        });
        let id = match graph.apply_strategy(strategy, new_id, config, origin) {
            Ok(id) => id,
            Err(e) => {
                self.refuse(format!("compose failed: {e}"));
                return None;
            }
        };
        // What it shares must be on disk before anything references it —
        // here, once, for every composition, and before the adopt re-points
        // `buffer`, after which an unsaved source exists nowhere.
        if let Err(why) = self.write_what_it_shares(&graph, id) {
            self.refuse(format!("compose: {why} — nothing composed"));
            return None;
        }
        // The working view is the whole composed transcript, rendered from the shared
        // turns (a straight merge owns nothing; a squash owns only its summary) —
        // whose bodies are loaded here, for the composed session: it exists only in
        // this graph, but every turn it slices is in the store (just above).
        if let Err(e) = shannon_core::hydrate(&*self.store, &mut graph, &[id]) {
            self.refuse(format!("compose: could not load the turns it shares: {e}"));
            return None;
        }
        let Some(full) = graph.records(&id) else {
            self.refuse("compose: part of the history it shares is not in the store");
            return None;
        };
        let mut composed = graph.episode(&id).cloned().expect("just composed");
        // Read before the landing, which rebuilds the buffer of what it
        // replaces.
        let mut said = None;
        if let Some((from, map)) = carries {
            let carried = self.marks_carried(from, map);
            said = carried.said("it", Some("their turns are not in it as they were"));
            carried.onto(&mut composed);
        }
        // Minted just now, so absent until written — by the landing, if it
        // owns new content (a squash's summary); else on first continue or
        // `:write`.
        //
        // **What it replaces is where it goes, read from what it carries** —
        // the carried session's own buffer, never the caller's `buffer`: `S`
        // on A's turns while aimed at B passed B's, and B's buffer became
        // A's squash, B left in none (full review 10-21, D 1, probed).
        let lands = match (carries, landing) {
            (Some((from, _)), _) => match self.buffer_for_episode(from) {
                Ok(home) => Lands::Over(home),
                Err(why) => {
                    self.refuse(format!("compose: {why} — nothing composed"));
                    return None;
                }
            },
            (None, Landing::Asked) => Lands::Beside(buffer),
            (None, Landing::Arrived) => Lands::Apart,
        };
        // Every window reading what it lands over, where it was (S21-Q4):
        // kept before the landing rebuilds it and puts them at its entry.
        let views = match (lands, carries) {
            (Lands::Over(home), Some((_, map))) => Some((home, self.views_on(home, map))),
            _ => None,
        };
        let landed = self.land(lands, landing, composed, full, OnDisk::Absent)?;
        if let Some((home, views)) = views {
            self.put_views(home, views);
        }
        // Landed: what it did not carry, said now (V 8).
        if let Some(said) = said {
            self.notify(said);
        }
        Some((id, landed))
    }

    /// **Land a composed `episode`** (its working view `turns`) as `lands`
    /// says, and say which buffer it is in now — the one door for every
    /// composition ([`Lands`]): `adopt_composed`'s, `:clone`, an edit's
    /// branch, `:import`. Only a turn delete rewrites a session in place
    /// otherwise (`adopt_session` directly), and it refuses while work runs.
    ///
    /// **And every landing's ending, here** (full review 10-21, D 5): seven
    /// verbs ended with `show_the_result`, two with only the trees rebuilt,
    /// so "a viewport is taken because you asked, never because an answer
    /// arrived" was held by each caller. `landing` says which: asked, the
    /// result is shown ([`show_the_result`](Self::show_the_result));
    /// arrived, the trees are rebuilt and no pane moves. Before that, a
    /// session landed `Over` carries the name of the one it goes on as
    /// ([`carry_label`](Self::carry_label)), and a session that owns new
    /// turns is written. What it says is the caller's
    /// ([`landed_notice`](Self::landed_notice)).
    ///
    /// **`None` where its secrets are password-protected and the password
    /// was not given** (`unlock`): nothing lands and nothing is replaced,
    /// said here; the caller says nothing more.
    pub(crate) fn land(
        &mut self,
        lands: Lands,
        landing: Landing,
        episode: Episode,
        turns: Vec<Turn>,
        on_disk: OnDisk,
    ) -> Option<BufferId> {
        let recall = match self.unlock_turns(&turns) {
            Ok(recall) => recall,
            Err(why) => {
                self.refuse(format!("{} not opened — {why}", episode.id.short_hex()));
                return None;
            }
        };
        // Read before the adopt: a replaced session that was never saved lives
        // only in the buffer the adopt is about to re-point.
        let replaces = match lands {
            Lands::Over(buffer) => self.session_of(buffer).map(|b| b.conversation.episode().id),
            Lands::Beside(_) | Lands::Into(..) | Lands::Apart => None,
        };
        let carried = replaces
            .map(|from| (from, self.label_of(from)))
            .filter(|(_, label)| !label.is_empty());
        let landed = self.place(lands, landing, episode, turns, on_disk, recall);
        if let Some((from, label)) = carried {
            self.carry_label(landed, from, label);
        }
        // After the carry, which writes it with its name when it carries one.
        self.write_if_it_owns(landed, replaces);
        match landing {
            Landing::Asked => self.show_the_result(landed),
            Landing::Arrived => self.refresh_graph_views(),
        }
        Some(landed)
    }

    /// Where [`land`](Self::land) puts the session: over a buffer, beside
    /// one (in it while it is blank), or apart — told of as `landing` says,
    /// wherever it lands (`App::hold`): an arrived summary was news `Over` a
    /// hidden buffer and silent `Apart` (review stop 9, A F6).
    fn place(
        &mut self,
        lands: Lands,
        landing: Landing,
        episode: Episode,
        turns: Vec<Turn>,
        on_disk: OnDisk,
        recall: super::Recall,
    ) -> BufferId {
        let (beside, into) = match lands {
            Lands::Over(buffer) => {
                self.adopt_session(buffer, episode, turns, on_disk, recall, landing);
                return buffer;
            }
            Lands::Beside(buffer) => (buffer, None),
            Lands::Into(buffer, pane) => (buffer, Some(pane)),
            Lands::Apart => {
                return self.buffer_holding(episode, turns, on_disk, recall, landing);
            }
        };
        // A blank buffer is taken over only where nobody could tell — and
        // opened `Into` a window, only if it is that window's own session:
        // the aim's blank buffer, reused for a tree's `<CR>` beside it, put
        // the import in the aim's pane too (V4b verify, M1).
        let unnoticed = self.unnoticed_if_reused(beside)
            && into.is_none_or(|pane| self.live_session(pane) == Some(beside));
        let landed = match unnoticed {
            true => {
                self.adopt_session(beside, episode, turns, on_disk, recall, landing);
                beside
            }
            false => self.buffer_holding(episode, turns, on_disk, recall, landing),
        };
        if let Some(pane) = into {
            self.enter_session(pane, landed);
            return landed;
        }
        if landed == beside {
            return landed;
        }
        // The window that showed the session it lands beside — visibly, else
        // the pane a viewer borrowed from it — shows the result; another pane
        // on that session keeps it. The aim follows where it was.
        if let Some(pane) = self.window_on(beside) {
            self.show_buffer_in(pane, landed);
        }
        if self.prompt_target() == beside {
            self.set_foreground(landed);
        }
        landed
    }

    /// **Where a composition went, said** — every composing verb's last
    /// sentence, `{what} → now in {noun} {tag} ({kept}; :graph to jump
    /// back)` where you are. Off the aim, named by whose work it was
    /// (`from`, [`notify_for`](Self::notify_for)). In no pane, the `:b N`
    /// first: the notice is its only sign, and the row cuts it from the end
    /// (full review 10-21, F 4, probed).
    ///
    /// One wording for seven verbs (D 5): a squash and `:continue` worked
    /// out where you were, and pick, merge, return and an edit assumed
    /// "now in", each with its own tail. Its consumers: `:clone`, pick,
    /// merge, return, an edit's branch, a squash, `:continue`. Not
    /// `:import`, whose sentence is an account of the conversation brought
    /// in (`imported_note`).
    pub(crate) fn landed_notice(
        &mut self,
        from: BufferId,
        landed: BufferId,
        what: &str,
        noun: &str,
        kept: &str,
    ) {
        let tag = self
            .session_of(landed)
            .map(|b| self.session_tag(b.conversation.episode().id))
            .unwrap_or_default();
        if self.continue_reaches(landed) {
            self.notify(format!(
                "{what} → now in {noun} {tag} ({kept}; :graph to jump back)"
            ));
        } else if self.panes_on(landed, super::Over::ThisTab).next().is_some() {
            // In this tab, or in no pane: a landing is placed in the event
            // that asked for it, in this tab's windows (`place`, `window_on`),
            // and its buffer is new — no other tab can show it (T2; the tab
            // site list verified `Lands::Into` synchronous).
            let body = format!("{what} → {noun} {tag} ({kept}; :graph to jump back)");
            self.notify_for(from, body);
        } else {
            let body = format!("{what} → {noun} {tag}, in no pane ({kept})");
            let line = format!(
                "`:b{}` shows it — {}",
                landed.number(),
                self.said_for(from, body)
            );
            self.notify(line);
        }
    }

    /// **Could `buffer` be rewritten without anyone telling?** Blank
    /// ([`is_blank`](Self::is_blank)) and in one pane at most, so the result
    /// shows where it was and nowhere else — `blank_buffer`'s rule, over a
    /// buffer.
    fn unnoticed_if_reused(&self, buffer: BufferId) -> bool {
        // In one window at most, in any tab: another tab's would change too.
        self.is_blank(buffer)
            && self
                .panes_with_session(buffer, super::Over::EveryTab)
                .count()
                <= 1
    }

    /// **Every session `episode` shares turns from, on disk** — the rule "a
    /// composition's sources are written first", held in one place
    /// (`adopt_composed`) for every composition. It was held per caller:
    /// `:fork` and `:compact` wrote first, `:clone` refused, and a squash,
    /// pick, merge or `:continue` of a session not yet written composed a
    /// reference to it — written at once when a name carried it, and read
    /// back after a relaunch a turn short, with no word said (review of
    /// 34033e8, Reviewers A and B, both reproduced).
    ///
    /// A source held by a buffer is written through `:w`'s door; one in no
    /// buffer and not on disk cannot be, and refuses.
    fn write_what_it_shares(&mut self, graph: &Graph, episode: EpisodeId) -> Result<(), String> {
        let mut sources: Vec<EpisodeId> = Vec::new();
        for segment in graph.episode(&episode).map_or(&[][..], |e| e.segments()) {
            if let Segment::Slice {
                episode: source, ..
            } = segment
                && !sources.contains(source)
            {
                sources.push(*source);
            }
        }
        for source in sources {
            if !matches!(self.store.episode_len(&source), Ok(None)) {
                continue;
            }
            let held = self
                .buffers
                .sessions()
                .find(|b| b.conversation.episode().id == source)
                .map(|b| b.id);
            let Some(holder) = held else {
                return Err(format!(
                    "it shares turns of {}, which is not saved and no longer open",
                    source.short_hex()
                ));
            };
            if matches!(
                self.persist_saying(holder, None),
                Saved::Refused | Saved::Gone
            ) {
                return Err(format!(
                    "{}, whose turns it shares, could not be written",
                    source.short_hex()
                ));
            }
        }
        Ok(())
    }

    /// `p` in `:graph`: **pick** — compose a new episode from the `<Space>`-marked nodes
    /// (design.md §1.2 base triad). Each marked turn/message contributes its turn's
    /// span; a marked episode contributes its whole history. They compose in graph
    /// (top-to-bottom) order via the [`Pick`] strategy — every turn shared by reference
    /// (COW), the sources untouched — and the result switches in as a new lazy session.
    /// A no-op (with a hint) if nothing's marked.
    pub(crate) fn pick_marked(&mut self) {
        // `:pick` reaches here from anywhere; `p` only from the tree. A
        // command that did nothing and said nothing read as broken (review
        // of 34033e8, Reviewer A).
        let Some(view) = self.graph_view() else {
            return self
                .refuse("pick composes the rows marked in `:graph` — open it and mark some");
        };
        let marked = view.chosen();
        if marked.is_empty() {
            self.refuse(
                "pick: select rows with `V` or mark them with <Space>, then `p` composes them",
            );
            return;
        }
        // The tree the marks were read from, which spends them.
        let tree = self.workspace.focused();
        let graph = self.current_graph();
        // Map each marked node to a span of its episode's derived sequence: a turn (or a
        // message) → that one turn; a whole episode → its full history.
        let picks: Vec<(EpisodeId, usize, usize)> = marked
            .iter()
            .map(|node| match *node {
                Node::Turn { episode, index, .. }
                | Node::Message {
                    episode,
                    turn_index: index,
                    ..
                } => (episode, index, index + 1),
                Node::Episode(id) => (id, 0, graph.full_turns(&id).len()),
            })
            .collect();
        // Origin (presentation): hang the composed session off the first pick's episode
        // + its first turn. `Pick` is N-ary; real provenance lives in the segments.
        let (base, ..) = picks[0];
        let (Some(fork_turn), Some(config)) = (
            graph.full_turns(&base).first().copied(),
            graph.episode(&base).map(|e| e.config.clone()),
        ) else {
            self.notify("pick: the first marked node's session is empty — nothing to pick");
            return;
        };
        let into = self.prompt_target();
        if let Some((id, landed)) = self.adopt_composed(
            into,
            graph,
            &Pick { picks },
            Anchor {
                base,
                turn: fork_turn,
                index: 0,
            },
            config,
            None,
            Landing::Asked,
        ) {
            self.spend_graph_marks(Some(tree), &marked);
            info!(picked = marked.len(), composed = %id.to_hex(), "picked marked nodes into a new session");
            let what = format!("picked {} nodes", marked.len());
            self.landed_notice(into, landed, &what, "session", "originals kept");
        }
    }

    /// `m` in `:graph`: **merge the selected branch straight into its base**
    /// (design.md §1.7). The branch's divergent turns splice in at the fork point,
    /// composing a NEW session (`A→B→X→Y→Z→C`); branch and base both preserved. One
    /// `MergeStraightIn` strategy over the shared [`Graph::compose`] engine.
    pub(crate) fn merge_branch_into_base(&mut self, branch: EpisodeId) {
        let graph = self.current_graph();
        let (base, fork_turn, fork_index, config) = match self.merge_target(&graph, branch) {
            Ok(t) => t,
            Err(reason) => {
                self.refuse(format!("merge: {reason}"));
                return;
            }
        };
        let into = self.prompt_target();
        if let Some((id, landed)) = self.adopt_composed(
            into,
            graph,
            &MergeStraightIn { base, branch },
            Anchor {
                base,
                turn: fork_turn,
                index: fork_index,
            },
            config,
            None,
            Landing::Asked,
        ) {
            info!(base = %base.to_hex(), branch = %branch.to_hex(), merged = %id.to_hex(), "merged branch straight into base");
            let what = format!(
                "merged {} into {}",
                self.session_tag(branch),
                self.session_tag(base)
            );
            self.landed_notice(into, landed, &what, "session", "originals kept");
        }
    }

    /// `r` in `:graph`: **return** the selected session's result into its parent
    /// (P6.3) — the child-session convenience over the shared compose engine.
    /// The whole line collapses to ONE turn appended to the parent: its first
    /// own user message (the briefing) in, its final assistant message out,
    /// `distilled_from` = every own turn, so the full working transcript stays
    /// one hop away in the inspector. The result turn carries **no usage** —
    /// composing is free; the spend stays recorded on the child's own turns
    /// (double-counting a subtree sum is the lie we refuse). Works on any
    /// session with a parent — a branch returns identically; the pattern
    /// doesn't care who the participants were. Originals kept; switches into
    /// the composed session, viewer refreshed.
    pub(crate) fn return_result(&mut self, episode: EpisodeId) {
        let graph = match self.graph_reading(&[episode]) {
            Ok(graph) => graph,
            Err(why) => return self.refuse(format!("return: {why}")),
        };
        let Some(ep) = graph.episode(&episode) else {
            self.refuse("return: that session is not in the graph");
            return;
        };
        let Some(origin) = ep.origin else {
            self.refuse(format!(
                "return: {} is a root — there is no parent to return into",
                self.session_tag(episode)
            ));
            return;
        };
        let own = ep.own_turns();
        if own.is_empty() {
            // The same value the border and `:graph` read, so a held child
            // is not described as running here either.
            let why = match self.activity_on(episode) {
                Some(crate::graph_view::Activity::Running) => {
                    "still running — nothing committed yet"
                }
                Some(crate::graph_view::Activity::Held) => {
                    "held on a command — deal with it or `:child --stop`"
                }
                None => "owns no turns (nothing to return)",
            };
            self.refuse(format!("return: {} {why}", self.session_tag(episode)));
            return;
        }
        // Briefing = the line's first own user message; result = its final own
        // assistant message. Own turns only — a branch's shared prefix belongs
        // to the parent already and is not part of what it "did".
        let Some(turns) = own
            .iter()
            .map(|t| graph.messages(t))
            .collect::<Option<Vec<&[Message]>>>()
        else {
            self.refuse("return: part of that session's turns is not in the store");
            return;
        };
        let briefing = turns
            .iter()
            .flat_map(|messages| messages.iter())
            .find(|m| m.role == Role::User)
            .map(|m| m.content.clone());
        let answer = turns
            .iter()
            .rev()
            .flat_map(|messages| messages.iter().rev())
            .find(|m| m.role == Role::Assistant)
            .map(|m| m.content.clone());
        let (Some(briefing), Some(answer)) = (briefing, answer) else {
            self.refuse(format!(
                "return: {} has no completed exchange to return",
                self.session_tag(episode)
            ));
            return;
        };
        let messages = vec![Message::user(briefing), Message::assistant(answer)];
        let handles = self
            .aliases
            .handles_to_record(&messages, &graph.recorded_handles(&own));
        // Assembled, not sent: no request produced this pair, so it seals as
        // authored — the child's preference carries an effort nothing sent.
        let result = Turn::seal(messages, ep.config.authored(), std::time::SystemTime::now())
            .with_distilled_from(own)
            .with_handles(handles);
        // The composed session extends the parent's END (the result arrives
        // "now"), so it anchors at the parent's head turn for presentation.
        let parent = origin.episode;
        let full = graph.full_turns(&parent);
        let (Some(&fork_turn), Some(fork_index)) = (full.last(), full.len().checked_sub(1)) else {
            self.refuse("return: the parent session has no turns to return into");
            return;
        };
        let config = graph
            .episode(&parent)
            .map(|e| e.config.clone())
            .unwrap_or_else(|| ep.config.clone());
        let into = self.prompt_target();
        if let Some((id, landed)) = self.adopt_composed(
            into,
            graph,
            &ResultInject {
                parent,
                result: result.clone(),
            },
            Anchor {
                base: parent,
                turn: fork_turn,
                index: fork_index,
            },
            config,
            None,
            Landing::Asked,
        ) {
            info!(
                from = %episode.to_hex(),
                parent = %parent.to_hex(),
                returned = %id.to_hex(),
                distilled = result.provenance.distilled_from.len(),
                "returned a session's result into its parent",
            );
            let what = format!(
                "returned {} into {}",
                self.session_tag(episode),
                self.session_tag(parent)
            );
            let kept = format!(
                "result distills {} turns; originals kept",
                result.provenance.distilled_from.len()
            );
            self.landed_notice(into, landed, &what, "session", &kept);
        }
    }

    /// `s` in `:graph`: **squash, hand-authored** (design.md §1.7/§1.2). With
    /// `<Space>` marks, squash the marked contiguous turn range in place
    /// ([`Squash`] — user-driven compaction); with no marks, squash-merge the
    /// selected branch into its base ([`SquashMerge`]: `A→B→P→C`, same shape as
    /// `m` with the branch's span replaced by the summary). Either way the
    /// summary is authored in `$EDITOR` as Messages-API JSON and composes
    /// through [`commit_summary`](Self::commit_summary) — the same tail the
    /// generated (`S`) path uses; only the seed differs.
    pub(crate) fn squash_selected_branch(&mut self, terminal: &mut ShannonTerminal) {
        let Some((into, plan)) = self.squash_by_hand() else {
            return;
        };
        let SquashPlan {
            graph,
            target,
            sources,
            msgs,
        } = plan;
        let seed_msgs = match &target {
            // Branch mode: group by role — all divergent user text, then all
            // assistant text — a summary-shaped draft the user condenses, with
            // the user message and the assistant response independently editable.
            SquashTarget::Branch { .. } => {
                let (mut user_parts, mut asst_parts): (Vec<&str>, Vec<&str>) =
                    (Vec::new(), Vec::new());
                for m in &msgs {
                    match m.role {
                        Role::User => user_parts.push(m.content.as_str()),
                        Role::Assistant => asst_parts.push(m.content.as_str()),
                    }
                }
                let mut seed = Vec::new();
                if !user_parts.is_empty() {
                    seed.push(Message::user(user_parts.join("\n\n")));
                }
                if !asst_parts.is_empty() {
                    seed.push(Message::assistant(asst_parts.join("\n\n")));
                }
                seed
            }
            // Range/gather mode: the marked turns verbatim, in order —
            // compaction/distillation starts from exactly what's there and the
            // user cuts it down.
            SquashTarget::Range { .. } | SquashTarget::Gather { .. } => msgs,
        };
        let seed = to_editable(&seed_msgs);
        // Save-to-apply: quitting without writing (`:q!`) cancels — the seed is
        // raw material here, not a decision.
        let summary_text = match crate::editor::edit_text_saved(
            terminal,
            self.mouse_captured,
            &seed,
            crate::editor::Filetype::Markdown,
        ) {
            Ok(Some(text)) => text,
            Ok(None) => {
                self.notify("squash cancelled: quit without saving");
                return;
            }
            Err(err) => {
                warn!(error = ?err, "squash cancelled; no change");
                self.notify("squash cancelled");
                return;
            }
        };
        // Typed by hand: no request produced it, so it seals as authored.
        self.commit_summary_into(into, graph, target, &sources, &summary_text, None);
    }

    /// `:squash`'s half before `$EDITOR` — the plan, and the buffer it lives
    /// in ([`squash_home`](Self::squash_home)), refused while that session is
    /// busy; `None` when refused, said. Apart so a test reaches it: no test
    /// can build the terminal the editor takes.
    ///
    /// It checked nothing, and composed over the aim: a squash of the aim's
    /// own early turns while its reply streamed ended the reply — silently
    /// if nothing had arrived, else "the turn you left was interrupted"
    /// (full review 10-21, D 1, probed).
    pub(crate) fn squash_by_hand(&mut self) -> Option<(BufferId, SquashPlan)> {
        let plan = match self.squash_plan() {
            Ok(plan) => plan,
            Err(reason) => {
                self.refuse(reason);
                return None;
            }
        };
        let home = match self.squash_home(&plan.target) {
            Ok(home) => home,
            Err(why) => {
                self.refuse(format!("`:squash`: {why}"));
                return None;
            }
        };
        if self.refuse_if_busy("squash", home) {
            return None;
        }
        Some((home, plan))
    }

    /// **Where a squash of `target` lives** — one answer for `:squash` and
    /// `S`/`:distill` (`:compact` resolves its session first and opens its
    /// buffer the same way): a range squashed in place, in its own session's
    /// buffer, opened if none is, since the result replaces that session; a
    /// branch's or a gather's, beside the prompt's aim, as it stands beside
    /// its sources. It is what the busy check asks about and, for `S`, the
    /// slot the distillation runs on, so CTRL-C where the range's session is
    /// stops it. Both used the aim, whichever session the range was in
    /// (full review 10-21, D 1).
    pub(crate) fn squash_home(&mut self, target: &SquashTarget) -> Result<BufferId, String> {
        match target {
            SquashTarget::Range { episode, .. } => self.buffer_for_episode(*episode),
            SquashTarget::Branch { .. } | SquashTarget::Gather { .. } => Ok(self.prompt_target()),
        }
    }

    /// `:compact [instructions]` / `:compact!` — **this session's history
    /// into one summary** (Steven, 10-14: "We'll need to implement that for
    /// daily usage"), and since 10-16 **as Claude Code's `/compact` does by
    /// default**: no review, the whole history summarised, and what follows
    /// the command is instructions for the summary — "I want the default
    /// `:compact` to act like Claude Code's default `/compact`". A squash of
    /// the session's own turns `[0, n-N)`, N being `[merge] compact_keep`
    /// (0), the model writing the summary (`[merge] squash_instruction`, as
    /// `S`, then the instructions) — reviewed in `$EDITOR` first only when
    /// `[merge] compact_review` says so, and `!` does the other for once.
    /// The result REPLACES the session (`carries`), so its name goes on with
    /// it; the original stays in `:graph`.
    ///
    /// On the session a bare session verb acts on
    /// ([`bare_session_target`](Self::bare_session_target)): the tree's pick
    /// with `:graph` focused, else the prompt's aim — composing in that
    /// session's own buffer, opened if it is not. One not on disk yet is
    /// written when the result composes, as every composition's sources are
    /// (`write_what_it_shares`), so nothing is written for a compaction that
    /// is refused or abandoned.
    pub(crate) fn compact_command(&mut self, args: &str, bang: bool) {
        // **A bare number was `:compact N`** — the last N turns kept — until
        // the words after it became the summary's instructions (10-16). Read
        // now as an instruction, `:compact 3` would summarise everything,
        // told "3", for whoever learned the old spelling: said instead. The
        // WHOLE argument only: after a flag or `--` the rest is words, as the
        // grammar says — `:compact -- 3` was refused, and `--keep 2 5` told to
        // be `--keep 5` (review stop 2, A F6 · B 4).
        let bare = args.trim();
        if !bare.is_empty() && bare.chars().all(|c| c.is_ascii_digit()) {
            // Echoed only when it can be one: `--keep 99999999999999999999`
            // would be refused in turn.
            let n = bare
                .parse::<usize>()
                .map_or("N".to_string(), |n| n.to_string());
            return self.refuse(format!(
                "`:compact` takes instructions for the summary, as Claude Code's `/compact` \
                 does — `:compact --keep {n}` keeps the last {n} turns as they were (`[merge] \
                 compact_keep`, {}, always); `:compact -- {bare}` sends it as words",
                self.config.merge.compact_keep
            ));
        }
        let (keep, instructions) = match compact_args(args) {
            Ok(read) => read,
            Err(why) => return self.refuse(why),
        };
        // Which said how many to keep, so a refusal names that one: `--keep 7`
        // on three turns blamed the setting (A F6 · B 4).
        let (keep, kept_by) = match keep {
            Some(n) => (n, format!("`--keep {n}`")),
            None => (
                self.config.merge.compact_keep,
                "`[merge] compact_keep`".to_string(),
            ),
        };
        let Some(episode) = self.bare_session_target(":compact") else {
            return;
        };
        let buffer = match self.buffer_for_episode(episode) {
            Ok(buffer) => buffer,
            Err(why) => return self.refuse(format!("`:compact`: {why}")),
        };
        if self.refuse_if_busy("compact", buffer) {
            return;
        }
        let plan = match self.compact_plan(episode, keep, &kept_by) {
            Ok(plan) => plan,
            Err(why) => return self.refuse(format!("`:compact`: {why}")),
        };
        let n = plan.sources.len();
        let tag = self.session_tag(episode);
        let review = self.config.merge.compact_review != bang;
        let extra = (!instructions.is_empty()).then_some(instructions);
        self.start_distill(buffer, plan, review, extra);
        let then = if review {
            self.review_when_written(buffer)
        } else {
            "it replaces them when it is written".to_string()
        };
        self.notify(format!(
            "compacting {n} turn{} of {tag} into a summary — {then}",
            if n == 1 { "" } else { "s" }
        ));
    }

    /// What `:compact` squashes: `episode`'s turns `[0, n-keep)`, in place —
    /// or why there is nothing to, naming what kept them (`kept_by`).
    fn compact_plan(
        &self,
        episode: EpisodeId,
        keep: usize,
        kept_by: &str,
    ) -> Result<SquashPlan, String> {
        let graph = self.graph_reading(&[episode])?;
        let full = graph.full_turns(&episode);
        let n = full.len();
        let end = n
            .checked_sub(keep)
            .filter(|end| *end > 0)
            .ok_or_else(|| match keep {
                0 => "nothing to compact — the session has no turns".to_string(),
                _ => format!(
                    "nothing to compact — {n} turn{}, and the last {keep} are kept ({kept_by})",
                    if n == 1 { "" } else { "s" }
                ),
            })?;
        let config = graph
            .episode(&episode)
            .ok_or("that session is not in the graph")?
            .config
            .clone();
        let sources = full[..end].to_vec();
        let msgs =
            collect_messages(&graph, &sources).ok_or("part of the history is not in the store")?;
        Ok(SquashPlan {
            target: SquashTarget::Range {
                episode,
                start: 0,
                end,
                fork_turn: full[0],
                config,
            },
            sources,
            msgs,
            graph,
        })
    }

    /// **What `s` and `S` squash, read from a graph that holds its words.**
    ///
    /// The one entry for both keys and their tests. It reads its own graph
    /// rather than taking one, because the target's messages ARE the summary's
    /// seed: a caller holding the plain snapshot (outlines only) used to be
    /// able to pass it, and `S`'s call site did exactly that under a mutation
    /// with no test noticing — every test squashed a session already open in a
    /// buffer, whose bodies the overlay supplies.
    pub(crate) fn squash_plan(&self) -> Result<SquashPlan, String> {
        let graph = self
            .squash_graph()
            .map_err(|why| format!("squash: {why}"))?;
        let (target, sources, msgs) = self.squash_target(&graph)?;
        Ok(SquashPlan {
            graph,
            target,
            sources,
            msgs,
        })
    }

    /// The graph a squash reads: the current one with the bodies of every
    /// session its marks or its selection name loaded — the summary's seed IS
    /// their messages. Private: [`squash_plan`](Self::squash_plan) is the way in.
    fn squash_graph(&self) -> Result<Graph, String> {
        let view = self.graph_view();
        let mut sessions: Vec<EpisodeId> = Vec::new();
        let named = view
            .map(GraphView::chosen)
            .unwrap_or_default()
            .into_iter()
            .chain(view.and_then(|v| v.selected_node()));
        for node in named {
            let session = node_episode(node);
            if !sessions.contains(&session) {
                sessions.push(session);
            }
        }
        self.graph_reading(&sessions)
    }

    /// Resolve what `s`/`S` should squash: the `<Space>`-marked range when marks
    /// exist, else the selected branch against its base. Returns the strategy
    /// target, the source turn ids (the summary's `distilled_from` provenance),
    /// and the source messages (the `$EDITOR` seed / the generation request) —
    /// or the user-facing reason the selection doesn't fit, which the caller
    /// surfaces via [`refuse`](Self::refuse): a refusal only the log can see
    /// reads as a dead key (a real dogfooding bug).
    fn squash_target(
        &self,
        graph: &Graph,
    ) -> Result<(SquashTarget, Vec<TurnId>, Vec<Message>), String> {
        // A range or the marks — what was picked out (`GraphView::chosen`).
        let marked = self.graph_view().map(GraphView::chosen).unwrap_or_default();
        if marked.is_empty() {
            return self.branch_squash_target(graph);
        }
        // Marks that read as one contiguous strip of a single timeline squash
        // IN PLACE; any other shape (cross-session, gaps, episodes mixed with
        // turns) falls through to a **gather** — distill the marked nodes into
        // a new summary-only session — instead of refusing.
        match marked_turn_range(&marked, graph) {
            Ok((episode, start, end)) => {
                let config = graph
                    .episode(&episode)
                    .ok_or("squash: that session is not in the graph")?
                    .config
                    .clone();
                let ids = graph.full_turns(&episode);
                let fork_turn = *ids
                    .get(start)
                    .ok_or("squash: the marked session has no turns")?;
                let sources: Vec<TurnId> = ids.get(start..end).unwrap_or_default().to_vec();
                let msgs = collect_messages(graph, &sources)
                    .ok_or("squash: part of the selection is not in the store")?;
                if msgs.is_empty() {
                    return Err("squash: the selection has no messages to squash".into());
                }
                Ok((
                    SquashTarget::Range {
                        episode,
                        start,
                        end,
                        fork_turn,
                        config,
                    },
                    sources,
                    msgs,
                ))
            }
            Err(_) => gather_target(&marked, graph),
        }
    }

    /// No-marks squash: the selected branch's divergent turns — everything past
    /// the fork (`full[k..]`), exactly the span [`SquashMerge`] replaces.
    fn branch_squash_target(
        &self,
        graph: &Graph,
    ) -> Result<(SquashTarget, Vec<TurnId>, Vec<Message>), String> {
        let node = self
            .graph_view()
            .and_then(|v| v.selected_node())
            .ok_or("squash: nothing selected")?;
        let branch = node_episode(node);
        let (base, fork_turn, fork_index, config) = self.merge_target(graph, branch)?;
        let k = graph.common_prefix_len(&base, &branch);
        let ids = graph.full_turns(&branch);
        if k >= ids.len() {
            return Err("squash: the branch has no divergent turns to squash".into());
        }
        let sources = ids[k..].to_vec();
        let msgs = collect_messages(graph, &sources)
            .ok_or("squash: part of the branch is not in the store")?;
        if msgs.is_empty() {
            return Err("squash: the selection has no messages to squash".into());
        }
        Ok((
            SquashTarget::Branch {
                base,
                branch,
                fork_turn,
                fork_index,
                config,
            },
            sources,
            msgs,
        ))
    }

    /// The composing half of a squash, shared by hand-authored `s` and generated
    /// `S`: parse the reviewed Messages-API JSON into turns (a multi-turn summary
    /// seals as multiple turns), record the distilled sources on each turn's
    /// provenance, and run the target's strategy through
    /// [`adopt_composed`](Self::adopt_composed). Malformed or empty input cancels
    /// cleanly (logged, nothing composes). Split from the `$EDITOR`-driving
    /// callers so the whole tail is testable without a terminal.
    ///
    /// **Two configs, never one** (Reviewers, 10-04): the summary TURNS are
    /// sealed under `seal` — what `S` sent the distillation under — or, typed
    /// by hand (`None`), as authored: no effort, no ceiling, none was sent.
    /// The composed SESSION keeps its base's own settings, read from `graph`
    /// at compose time. One value for both made an `S` session adopt what was
    /// sent as its preference: the run's effort pinned, the stub's model with
    /// no key.
    pub(crate) fn commit_summary_into(
        &mut self,
        buffer: BufferId,
        graph: Graph,
        target: SquashTarget,
        sources: &[TurnId],
        edited: &str,
        seal: Option<TurnConfig>,
    ) -> Option<EpisodeId> {
        self.compose_summary_into(buffer, graph, target, sources, edited, seal, Landing::Asked)
    }

    /// [`commit_summary_into`](Self::commit_summary_into), told how the
    /// result reaches the screen ([`Landing`]) — the one caller that is not
    /// answering a keystroke is an unreviewed `:compact`'s summary,
    /// composing when the model is done.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn compose_summary_into(
        &mut self,
        buffer: BufferId,
        graph: Graph,
        target: SquashTarget,
        sources: &[TurnId],
        edited: &str,
        seal: Option<TurnConfig>,
        landing: Landing,
    ) -> Option<EpisodeId> {
        // **The target is re-checked against the graph it composes into.** An
        // `S` distillation resolves its positions at the keypress and composes
        // when the summary comes back; a turn deleted in between shifts them,
        // and the summary replaced the wrong turns — keeping one beside its
        // own summary and dropping another (Reviewer A, round 18, reproduced).
        if let Some(why) = stale_target(&graph, &target, sources) {
            self.refuse(format!(
                "summary not applied: {why} — nothing composed; ask for it again"
            ));
            return None;
        }
        let turn_msgs = match from_editable_turns(edited) {
            // Masked before it is sealed: the summary is text the user wrote.
            Ok(t) if !t.is_empty() => self.mask_authored(t),
            Ok(_) => {
                self.notify("squash cancelled: empty summary");
                return None;
            }
            Err(err) => {
                self.refuse(format!(
                    "squash cancelled: could not parse the edit ({err})"
                ));
                return None;
            }
        };
        let config = match &target {
            SquashTarget::Branch { config, .. }
            | SquashTarget::Range { config, .. }
            | SquashTarget::Gather { config, .. } => config.clone(),
        };
        let sealed = seal.unwrap_or_else(|| config.authored());
        let now = std::time::SystemTime::now();
        let held = std::sync::Arc::clone(&self.aliases);
        let vouched = graph.recorded_handles(sources);
        let summary: Vec<Turn> = turn_msgs
            .into_iter()
            .map(|m| {
                let handles = held.handles_to_record(&m, &vouched);
                Turn::seal(m, sealed.clone(), now)
                    .with_distilled_from(sources.to_vec())
                    .with_handles(handles)
            })
            .collect();
        // How many turns the summary is — a multi-turn summary seals as
        // several — read before it moves into the strategy: the marks after
        // the squash move by it (`squashed`).
        let summary_turns = summary.len();
        // The session the turns came from, by its tag BEFORE the result takes
        // its buffer — after, that buffer names the result, and the notice
        // named only the result (full review 10-21, F 6, probed).
        let from = match &target {
            SquashTarget::Range { episode, .. } => Some(self.session_tag(*episode)),
            SquashTarget::Branch { branch, .. } => Some(self.session_tag(*branch)),
            SquashTarget::Gather { .. } => None,
        };
        // The last field: whether the result REPLACES a session (its own
        // turns squashed — it carries the name on) or stands beside two (a
        // branch folded into its base — neither one's name is its).
        let (base, fork_turn, fork_index, strategy, carries): (_, _, _, Box<dyn MergeStrategy>, _) =
            match target {
                SquashTarget::Branch {
                    base,
                    branch,
                    fork_turn,
                    fork_index,
                    ..
                } => (
                    base,
                    fork_turn,
                    fork_index,
                    Box::new(SquashMerge {
                        base,
                        branch,
                        summary,
                    }),
                    None,
                ),
                SquashTarget::Range {
                    episode,
                    start,
                    end,
                    fork_turn,
                    ..
                } => (
                    episode,
                    fork_turn,
                    start,
                    Box::new(Squash {
                        episode,
                        start,
                        end,
                        summary,
                    }),
                    Some((episode, start, end)),
                ),
                SquashTarget::Gather {
                    episode,
                    turn,
                    index,
                    ..
                // A NEW session holding only the summary, its sources left
                // whole — it replaces nothing, so no name follows it
                // (review of 34033e8, Reviewer C: the first marked session's
                // name moved onto the gather and left the session it named).
                } => (
                    episode,
                    turn,
                    index,
                    Box::new(GatherSummary { summary }),
                    None,
                ),
            };
        // The base's settings as they are NOW — an `:effort` set while `S`
        // ran carries over.
        let preference = graph.episode(&base).map_or(config, |e| e.config.clone());
        // Its marks on the turns before the squash and after it go on; those
        // on the turns it folded are said, not carried (`marks_carried`).
        let map = carries.map(|(_, start, end)| squashed(start, end, summary_turns));
        let carries = (carries.zip(map.as_ref()))
            .map(|((from, ..), map)| (from, map as super::marks::TurnMap<'_>));
        // A result that arrives later — an unreviewed `:compact`, or an `S`
        // with `distill_review` off — replaces its session when it carries
        // one, and otherwise stands apart, in no pane (`Lands::Apart`).
        let (id, landed) = self.adopt_composed(
            buffer,
            graph,
            strategy.as_ref(),
            Anchor {
                base,
                turn: fork_turn,
                index: fork_index,
            },
            preference,
            carries,
            landing,
        )?;
        info!(
            base = %base.to_hex(),
            squashed = %id.to_hex(),
            distilled = sources.len(),
            "squashed into a new session",
        );
        // Say what just happened and where the user now IS — silently switching
        // into a freshly-composed session reads as "landed somewhere weird."
        // **Where the user is**, though: a summary arriving while you are
        // aimed at another session landed in that one's, and "now in
        // session" was false of you (full review 10-21, A 10, probed). One
        // in no pane — standing apart (`Lands::Apart`), or over a session no
        // pane shows (D 1: a range's own buffer, opened for it) — is named by
        // the session it was made from, `buffer`, carried from the keypress:
        // this is also reached after an `S` review, the aim moved since.
        let n = sources.len();
        let turns = if n == 1 { "turn" } else { "turns" };
        let of = from.map(|from| format!(" of {from}")).unwrap_or_default();
        let what = format!("squashed {n} {turns}{of}");
        self.landed_notice(buffer, landed, &what, "session", "originals kept");
        Some(id)
    }

    /// **Check out a past turn** — go back to that point in the log (git
    /// detached-HEAD style). The transcript shows the episode's turns **up to and
    /// including** `turn`; continuing from here forks a branch, leaving the
    /// original log past this point untouched. Checking out the *tip* is just
    /// opening the whole session. The checkout is a **lazy, unpersisted** branch:
    /// it leaves no trace unless you continue (then it persists as a real branch).
    /// The **persisted** episode a new branch may hang under: `episode` itself
    /// when it is on disk, else the source of a pure clone (one that has added
    /// nothing of its own, so its content really is the source's).
    ///
    /// `None` when neither holds; the caller words the refusal, because the two
    /// callers are different verbs to the user and a shared string would have to
    /// be vague about both. `verb` is for the log line only.
    ///
    /// One resolver because it is one rule: a slice that references an episode
    /// nobody wrote down is a dangling reference the next load cannot follow.
    /// It was stated identically at two call sites, which is the shape this file
    /// already flags at `clone_episode` and did not act on
    /// (`[[feedback_rules_rot_by_retyping]]`, Reviewer C round 3). No behaviour
    /// change — the two copies agreed today, and the point is that nothing was
    /// making them.
    fn persisted_anchor(
        &self,
        episode: EpisodeId,
        source: &Episode,
        verb: &str,
    ) -> Option<EpisodeId> {
        // Existence only — `episode_len` reads no body. A store that cannot say
        // is not proof of a write, so it does not anchor; the log keeps why.
        match self.store.episode_len(&episode) {
            Ok(Some(_)) => return Some(episode),
            Ok(None) => {}
            Err(e) => {
                warn!(episode = %episode.to_hex(), error = %e, "cannot tell if it is on disk")
            }
        }
        let clone_source = source
            .origin
            .filter(|o| o.kind == EdgeKind::Clone && source.own_is_empty())
            .map(|o| o.episode);
        if clone_source.is_none() {
            warn!(episode = %episode.to_hex(), verb, "cannot {verb} an unpersisted session");
        }
        clone_source
    }

    pub(crate) fn checkout_turn(&mut self, episode: EpisodeId, turn: TurnId, turn_index: usize) {
        // Read the live snapshot (store + active overlay) so checking out a turn of
        // an unpersisted clone works — its turns resolve through the source it
        // shares — with the bodies of that session loaded, for the working view.
        let graph = match self.graph_reading(&[episode]) {
            Ok(graph) => graph,
            Err(why) => return self.refuse(format!("checkout: {why}")),
        };
        let Some(source) = graph.episode(&episode).cloned() else {
            warn!(episode = %episode.to_hex(), "could not find the episode to check out");
            return self.refuse("checkout: that session is not in the graph");
        };
        let full_ids = graph.full_turns(&episode);
        // `turn_index` is the selected node's occurrence index — validate it in range
        // and that it still names `turn` (defensive against a stale selection); never
        // re-search by content, which would pick the wrong repeat
        // ([[content-addressed-ids]]).
        if full_ids.get(turn_index) != Some(&turn) {
            warn!(
                turn_index,
                "checkout: stale selection (index/turn mismatch)"
            );
            return self.refuse("checkout: that turn moved — reopen `:graph` and pick it again");
        }
        // Checking out the tip = open the whole session as-is — at its end,
        // the turn picked, as every other turn's checkout lands (S21-Q5).
        if turn_index + 1 == full_ids.len() {
            if let Some(pane) = self.switch_session(episode) {
                self.at_its_end(pane);
            }
            return;
        }
        // The origin (presentation) hangs the checkout under a *persisted*
        // episode — see `persisted_anchor`.
        let Some(anchor) = self.persisted_anchor(episode, &source, "check out") else {
            // `<CR>` in `:graph` reaches this, and reporting it only to the log
            // made the key read as broken ([[feedback_visible_refusals]]).
            return self
                .refuse("checkout: this session isn't on disk yet — send a turn in it first");
        };
        // A detached checkout: a lazy branch that *shares* the history up to and
        // including this turn and owns nothing yet. Its segments come from `project`,
        // referencing the true data OWNERS — so they stay durable even when `episode`
        // is a lazy clone/checkout (not itself on disk). Continuing commits it as a
        // real branch (persist-on-first-turn); just looking and leaving discards it.
        // The working view still gets the full prefix transcript to render.
        let segments = graph.project(&episode, 0, turn_index + 1);
        let Some(mut prefix) = graph.records(&episode) else {
            return self.refuse("checkout: part of that session's history is not in the store");
        };
        prefix.truncate(turn_index + 1);
        let branch_id = fresh_episode_id();
        let mut branch_ep = Episode::from_segments(
            branch_id,
            source.config.clone(),
            segments,
            Some(Origin {
                episode: anchor,
                turn,
                index: Some(turn_index),
                kind: EdgeKind::Branch,
            }),
        );
        // The marks on the turns it shares, as its own (S16-Q2); one past
        // the turn checked out is not in its history, and not missed.
        let carried = self.marks_carried(episode, &|at| (at <= turn_index).then_some(at));
        let said = carried.said("the checkout", None);
        carried.onto(&mut branch_ep);
        // **A buffer of its own, entered as `:open <session>` enters one** —
        // in place, as vim's `:e`: the window `:open` lands in shows it, and
        // what that window showed stays a buffer (`:b#`), its work running on
        // (Steven, 10-16, D3: "If I type `:e /the/file/I/want` then I don't
        // split or do anything weird, I just change what I'm looking at").
        //
        // It ADOPTED the checkout into the focused pane's buffer, which
        // displaced the session there — superseding its work, and, if it
        // was never written, losing it — and from a lent viewer that was the
        // session BENEATH the viewer, not the aim every other session verb
        // acts on (review stop 2, A F5). And `:open` reaches this or
        // `switch_session` purely by which id space the hash resolved into,
        // so the two end through the same two steps: one verb, one landing.
        // A lazy checkout: minted just now, so absent until continued or
        // `:write`.
        let recall = match self.unlock_turns(&prefix) {
            Ok(recall) => recall,
            Err(why) => return self.refuse(format!("checkout: not opened — {why}")),
        };
        let into = self.buffer_holding(branch_ep, prefix, OnDisk::Absent, recall, Landing::Asked);
        if let Some(checkout) = self.session_of_mut(into) {
            checkout.made_as_a_look();
        }
        let pane = self.session_host(into);
        // At its end — the turn you picked — not where you were reading:
        // picking a turn is picking the place (S21-Q5, Steven 11-01: "it is
        // correct to land on the turn you are in"), and a carry made it land
        // two ways, by whether the graph's pane had been reading the session.
        // A new session, so the window enters it at its entry: its end
        // (`SessionBuffer::entry`), as the tip's lands (`App::at_its_end`).
        self.enter_session(pane, into);
        // **Said where you are** (review stop 15, F M7): what a checkout is
        // went to the log alone, and the title said "branch" of a session
        // nothing was sent in (Steven's observation 3; S21-Q8, `at`).
        let landed = format!(
            "at {} (turn {} of {}) — sending continues from here as a branch; leaving keeps \
             nothing",
            turn.short_hex(),
            turn_index + 1,
            source.handle()
        );
        self.notify(match said {
            Some(said) => format!("{landed} · {said}"),
            None => landed,
        });
        info!(
            episode = %episode.to_hex(),
            anchor = %anchor.to_hex(),
            turn_index,
            "checked out a past turn (detached; continuing will branch)"
        );
    }

    /// `e` in `:graph`: **edit the conversation up to the selected point**, then fork
    /// a branch from the edited history (design.md §1.2). The whole prefix — every
    /// turn up to and including the selection — is rendered as the **Messages-API JSON**
    /// ([`to_editable`]) into `$EDITOR` (the real on-the-wire shape, editable in full),
    /// so both the user message and the assistant response of each turn are editable. On
    /// save it's parsed back into turns ([`from_editable_turns`]) and re-sealed.
    ///
    /// The branch is **copy-on-write**: it shares BOTH unchanged flanks by reference
    /// (content-address match, re-sealing under each turn's *original* config) and owns
    /// only the edited *middle* — a pointer is cheaper than the data. Turns *after* the
    /// selected point are dropped (you branch at the point); the original is untouched.
    ///
    /// A no-op (with a hint) if the selection has no history, the edit is unchanged,
    /// or the buffer is emptied. Leaves the viewer once composing. Best-effort: a
    /// load/persist failure logs and leaves the current session in place.
    pub(crate) fn edit_selected_node(&mut self, terminal: &mut ShannonTerminal) {
        let EditPlan {
            graph,
            episode,
            start,
            end,
            message_scope,
            seed,
        } = match self.edit_plan() {
            Ok(plan) => plan,
            Err(why) => return self.refuse(why),
        };
        let edited = match crate::editor::edit_text(
            terminal,
            self.mouse_captured,
            &seed,
            crate::editor::Filetype::Markdown,
        ) {
            Ok(text) => text,
            Err(err) => {
                warn!(error = ?err, "edit cancelled; no branch");
                self.notify("edit cancelled");
                return;
            }
        };
        if edited == seed {
            self.notify("edit unchanged; nothing composed");
            return;
        }
        // A message-level edit splices back into ITS turn, then rides the same
        // turn-slice path as everything else.
        let slice_text = match message_scope {
            Some((turn, index)) => match spliced_message_edit(&graph, turn, index, &edited) {
                Ok(text) => text,
                Err(reason) => {
                    self.refuse(reason);
                    return;
                }
            },
            None => edited,
        };
        self.commit_slice_edit(episode, start, end, &slice_text);
    }

    /// The reading half of [`edit_selected_node`]: the row under the cursor as
    /// a slice of a session, and the seed `$EDITOR` opens on. Split from the
    /// editor so the read is testable — the seed IS the turns' words, so it
    /// reads them from a graph with that session's bodies loaded, and a call
    /// site that skipped the load had no test able to notice.
    pub(crate) fn edit_plan(&self) -> Result<EditPlan, String> {
        // What every graph verb acts on, not the cursor alone: with a range or
        // marks on screen, editing the cursor row and ignoring them reads as
        // having edited all of them.
        let targets = self
            .graph_view()
            .map(GraphView::acts_on)
            .unwrap_or_default();
        let node = match targets.as_slice() {
            [] => return Err("edit: nothing selected".into()),
            [node] => *node,
            many => {
                return Err(format!(
                    "edit: {} rows are selected and `e` edits one — `Esc` or `mu`, then `e` on it",
                    many.len()
                ));
            }
        };
        let graph = self
            .graph_reading(&[node_episode(node)])
            .map_err(|why| format!("edit: {why}"))?;
        // WYSIWYG, at every level of the hierarchy: the row you're on is what you
        // edit. A MESSAGE row edits that one message; a TURN row edits that one
        // turn; an EPISODE row edits the turns displayed under it (its own turns —
        // the whole conversation for a root). The unedited flanks — before AND
        // after — stay untouched and shared by reference; dropping a suffix is now
        // a deliberate act (episode-row edit, delete the trailing turns).
        let (episode, start, end) = match node {
            Node::Turn { episode, index, .. } => (episode, index, index + 1),
            Node::Message {
                episode,
                turn_index,
                ..
            } => (episode, turn_index, turn_index + 1),
            Node::Episode(id) => marked_turn_range(&[Node::Episode(id)], &graph)
                .map_err(|reason| format!("edit: {reason}"))?,
        };
        let full_ids = graph.full_turns(&episode);
        if start >= end || full_ids.len() < end {
            return Err("edit: the selection is out of range".into());
        }
        // The seed is exactly the selected scope: one message, or the slice's
        // messages. `commit_slice_edit` re-derives the originals, so read-only.
        let message_scope = match node {
            Node::Message { turn, index, .. } => Some((turn, index)),
            _ => None,
        };
        let seed_msgs: Vec<Message> = match message_scope {
            Some((turn, index)) => vec![
                graph
                    .messages(&turn)
                    .and_then(|m| m.get(index))
                    .ok_or("edit: that message is not in the graph")?
                    .clone(),
            ],
            None => collect_messages(&graph, &full_ids[start..end])
                .ok_or("edit: part of that history is not in the store")?,
        };
        Ok(EditPlan {
            seed: to_editable(&seed_msgs),
            graph,
            episode,
            start,
            end,
            message_scope,
        })
    }

    /// The composing half of [`edit_selected_node`], split out so the edit→branch
    /// logic is testable without `$EDITOR`. Replace turns `[start, end)` of
    /// `episode`'s derived sequence with the edited Messages-API JSON — both flanks
    /// stay untouched — and fork a **copy-on-write** branch via
    /// [`Graph::reconstruct`](shannon_core::Graph::reconstruct): every turn that already
    /// exists anywhere in the graph is shared by reference, only genuinely-new turns are
    /// owned, and the diff decides the op (branch / re-root / prune / cross-branch reuse).
    /// An edited slice that parses to NO turns **deletes** the slice (the flanks meet).
    /// Returns the new branch id, or `None` (surfaced) on a no-op / malformed edit /
    /// failure. Leaves the `:graph` viewer.
    pub(crate) fn commit_slice_edit(
        &mut self,
        episode: EpisodeId,
        start: usize,
        end: usize,
        edited: &str,
    ) -> Option<EpisodeId> {
        // Read the live snapshot (store + overlay) so editing an unpersisted clone
        // works — its turns resolve through the source it shares — with this
        // session's bodies loaded: the edit compares and keeps whole records.
        let graph = match self.graph_reading(&[episode]) {
            Ok(graph) => graph,
            Err(why) => {
                self.refuse(format!("edit: {why}"));
                return None;
            }
        };
        let source = graph.episode(&episode).cloned()?;
        let full_ids = graph.full_turns(&episode);
        // Defensive (per AGENTS.md "a panic that depends on input is a bug"):
        // an empty episode has no turn to anchor the edit's fork point, and the
        // `full_ids.len() - 1` below would underflow on it. The live caller
        // (`edit_selected_node`) can't reach this — you can't select a turn in a
        // turn-less session — but this is `pub(crate)` and directly tested, so
        // it defends the case its own bounds otherwise permit (`start == end ==
        // 0`, which passes the range check on an empty slice).
        if start > end || full_ids.len() < end || full_ids.is_empty() {
            self.notify("edit: slice out of range");
            return None;
        }
        // The session as records — each position's content with ITS
        // occurrence's facts — and the slice's originals among them.
        let Some(records) = graph.records(&episode) else {
            self.refuse("edit: part of this session's history is not in the store");
            return None;
        };
        let originals: Vec<Turn> = records[start..end].to_vec();

        // Parse the edited Messages-API JSON into turns; malformed JSON → cancel.
        // Masked before anything is sealed: the edit is text the user wrote.
        // An untouched turn is matched to its original masked alike
        // (`as_masked`, below): the original was stored as the model wrote it,
        // and masking its text can change it (a plain alias, the home's `~`).
        let edited_msgs = match from_editable_turns(edited) {
            Ok(t) => self.mask_authored(t),
            Err(err) => {
                warn!(error = %err, "edit cancelled: could not parse the edited transcript");
                self.notify(format!("edit cancelled: could not parse ({err})"));
                return None;
            }
        };
        // Re-seal each edited turn under its ORIGINAL slot's config, so an untouched
        // turn reproduces its content-address; turns added beyond the original
        // slice take the session config.
        //
        // **An untouched turn IS its original** — the same production, so it keeps
        // its record and facts, and `reconstruct` shares that occurrence instead of
        // owning a copy stamped `now` (which would also drop its usage). The same
        // slot first, then any original not yet claimed, so a reorder keeps both
        // turns' facts. Only text the user actually changed is a new production.
        let mut claimed = vec![false; originals.len()];
        let held = std::sync::Arc::clone(&self.aliases);
        let sources: Vec<TurnId> = originals.iter().map(|t| t.id).collect();
        let vouched = graph.recorded_handles(&sources);
        // **Each original as this door masks it** — the edit is masked as you
        // wrote it (`mask_authored`: every alias, the home's `~` among them),
        // the original stored as the model wrote it. Compared bare, a turn
        // saved unchanged differed wherever an alias names its text, and an
        // edit of nothing made a branch (review stop 15, E H1).
        let as_masked: Vec<TurnId> = (originals.iter())
            .map(|o| {
                let messages = o.messages.iter().map(|m| held.mask_message(m).0).collect();
                Turn::seal(messages, o.provenance.config.clone(), o.provenance.at).id
            })
            .collect();
        let is =
            |j: usize, sealed: &Turn| originals[j].id == sealed.id || as_masked[j] == sealed.id;
        let edited_slice: Vec<Turn> = edited_msgs
            .into_iter()
            .enumerate()
            .map(|(i, messages)| {
                // A turn the edit ADDS was typed, not sent: authored.
                let config = originals
                    .get(i)
                    .map(|t| t.provenance.config.clone())
                    .unwrap_or_else(|| source.config.authored());
                let handles = held.handles_to_record(&messages, &vouched);
                let sealed = Turn::seal(messages, config, std::time::SystemTime::now())
                    .with_handles(handles);
                let same_slot = i < originals.len() && !claimed[i] && is(i, &sealed);
                let original = if same_slot {
                    Some(i)
                } else {
                    (0..originals.len()).find(|&j| !claimed[j] && is(j, &sealed))
                };
                match original {
                    Some(j) => {
                        claimed[j] = true;
                        originals[j].clone()
                    }
                    None => sealed,
                }
            })
            .collect();
        // No-op: the edited slice reproduces the original slice exactly (ids match
        // positionally). A reorder or a pure deletion changes the sequence, so it
        // counts as a real change even when nothing new is authored.
        if edited_slice.len() == originals.len()
            && edited_slice
                .iter()
                .zip(&originals)
                .all(|(e, o)| e.id == o.id)
        {
            self.notify("edit unchanged; nothing composed");
            return None;
        }
        // The full target: untouched prefix ++ edited slice ++ untouched suffix.
        let target: Vec<Turn> = records[..start]
            .iter()
            .cloned()
            .chain(edited_slice.iter().cloned())
            .chain(records[end..].iter().cloned())
            .collect();
        if target.is_empty() {
            self.notify("edit removed every turn — use :delete to drop a session");
            return None;
        }

        // Hang the branch under a PERSISTED episode — see `persisted_anchor`.
        let Some(anchor) = self.persisted_anchor(episode, &source, "edit") else {
            self.notify("edit: this session isn't persisted yet — send a message first");
            return None;
        };
        // Fork turn (presentation): the original turn where the edit first diverges.
        let head = edited_slice
            .iter()
            .zip(&originals)
            .take_while(|(e, o)| e.id == o.id)
            .count();
        let fork_index = (start + head).min(full_ids.len() - 1);
        let fork_turn = full_ids[fork_index];

        // Universal reconstruction: share every turn that already exists ANYWHERE in the
        // graph by reference (content-addressed), own only genuinely-new turns. Branch /
        // re-root / prune / cross-branch reuse all fall out of where the boundaries land.
        // Lower each `Span` via `project` (resolved to the true owners, so cross-episode
        // sharing is durable); each `New` run becomes an owned segment + turns to persist.
        let mut segments: Vec<Segment> = Vec::new();
        let mut owned: Vec<Turn> = Vec::new();
        for piece in graph.reconstruct(&target) {
            match piece {
                Piece::Span {
                    episode,
                    start,
                    end,
                } => segments.extend(graph.project(&episode, start, end)),
                Piece::New(turns) => {
                    segments.push(Segment::Own(turns.iter().map(Turn::occurrence).collect()));
                    owned.extend(turns);
                }
            }
        }
        // View history: the target itself — every record already carries the facts
        // of its own occurrence (a shared turn its original's, a new one its own).
        let view_turns: Vec<Turn> = target;

        let branch_id = fresh_episode_id();
        let mut branch_ep = Episode::from_segments(
            branch_id,
            source.config.clone(),
            segments,
            Some(Origin {
                episode: anchor,
                turn: fork_turn,
                index: Some(fork_index),
                kind: EdgeKind::Branch,
            }),
        );
        // The marks on the turns the edit left as they were, as its own
        // (S16-Q2) — the suffix's moved by the slice's change in length;
        // those on the turns it changed said, not carried.
        let map = edited_turns(start, start + head, end, edited_slice.len());
        let carried = self.marks_carried(episode, &map);
        let said = carried.said("the edit", Some("their turns changed"));
        carried.onto(&mut branch_ep);
        // Persist the owned turns eagerly (header + a Slice per shared run + the owned
        // turns, atomically). An all-shared result (prune / reorder) owns nothing → lazy.
        let on_disk = if owned.is_empty() {
            OnDisk::Absent // minted just above; written on continue or `:write`
        } else {
            if let Err(e) = self.persist_episode(&branch_ep, &owned) {
                warn!(episode = %branch_id.to_hex(), error = %e, "could not persist the edit; aborting");
                // The one failure in this function that loses text the user
                // just authored in `$EDITOR` — and the only one that said
                // nothing. You save, quit, and land back in a TUI where nothing
                // changed and nothing is explained ([[feedback_visible_refusals]]).
                self.refuse(format!("edit NOT saved — could not write the branch: {e}"));
                return None;
            }
            // The edit lands where you are (`into`, below), so it is `HEAD` —
            // by `moves_head`'s rule, written before it adopts; said if it
            // cannot be (it was in the log alone).
            self.set_head(branch_id);
            OnDisk::Own(branch_ep.own_len())
        };
        let from = self.prompt_target();
        // Refused by the password, the branch is still what it is — written
        // above when it owns turns — unopened (`land` says so).
        let Some(landed) = self.land(
            Lands::Beside(from),
            Landing::Asked,
            branch_ep,
            view_turns,
            on_disk,
        ) else {
            return Some(branch_id);
        };
        // Where you were reading it, on the turns it left as they were
        // (S21-Q4).
        self.carry_into_shown(episode, landed, &map);
        info!(
            episode = %branch_id.to_hex(),
            owned = owned.len(),
            "reconstructed edited history into a branch"
        );
        self.landed_notice(from, landed, "edited", "branch", "original kept");
        // Landed: what it did not carry, said now (V 8).
        if let Some(said) = said {
            self.notify(said);
        }
        Some(branch_id)
    }
}
