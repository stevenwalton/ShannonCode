//! **Deleting sessions and turns** — what `d`, `:delete` and `:delete
//! all` resolve to and the question they ask; a buffer reset; holders
//! given their own copies first; the store rewritten; the sentence that
//! says what went. Moved out of `app.rs` whole (review stop 2, D7).

use super::*;

impl App {
    /// Resolve what `d` will delete, or `None` with the refusal already shown.
    ///
    /// **`d` deletes turns.** It used to refuse any selection containing a turn
    /// row, which was written for a narrower bug — `d` on a turn silently
    /// escalating to the whole session — and then generalised past what anyone
    /// decided. Refusing the escalation was right; refusing the operation meant
    /// a user who marked a range of sessions could not delete it because one
    /// row in it happened to be a turn (Steven, drive round 4).
    ///
    /// The rule, in three parts:
    ///
    /// - **Sharing never refuses.** Another session reading past the cut keeps
    ///   its history: it is given its own copy first
    ///   ([`detach_holders`](Self::detach_holders)), at the moment of the
    ///   write. It refused here once, and a forked conversation then could not
    ///   be deleted from the tree at all (Steven, 10-04).
    /// - **Warn on a GAP, do not block.** Removing a turn that something later
    ///   still answers leaves a reply to a question that is gone. That is
    ///   semantic, not structural: the history is well-formed, it just reads
    ///   oddly. So it raises the confirmation to [`Alarm::Grave`] and says so,
    ///   and the user decides ([[project_shannon_no_silent_fallback]]).
    /// - **A trailing run is ordinary.** Nothing follows it, so nothing can be
    ///   left dangling and there is nothing to warn about.
    ///
    /// Turns inside a session that is *also* being deleted drop out, so the
    /// count the user approves is the count that happens.
    pub(super) fn delete_targets(&mut self) -> Option<ConfirmAction> {
        let nodes = self.graph_nodes("delete")?;
        // Resolved once for the whole resolution: the length each turn reports,
        // and the gap check below, both read it.
        let graph = self.current_graph();
        let mut episodes: Vec<EpisodeId> = Vec::new();
        let mut turns: Vec<TurnTarget> = Vec::new();
        let mut messages: Vec<(EpisodeId, usize)> = Vec::new();
        for node in nodes {
            match node {
                Node::Episode(id) => {
                    if !episodes.contains(&id) {
                        episodes.push(id);
                    }
                }
                Node::Turn {
                    episode,
                    turn,
                    index,
                } => {
                    if !turns
                        .iter()
                        .any(|t| t.episode == episode && t.index == index)
                    {
                        turns.push(TurnTarget {
                            episode,
                            index,
                            turn,
                            of_total: graph.full_turns(&episode).len(),
                        });
                    }
                }
                Node::Message {
                    episode,
                    turn_index,
                    ..
                } => messages.push((episode, turn_index)),
            }
        }
        // A message is not a unit of history here: turns are what the graph
        // composes, and "delete half a turn" has no piece-table meaning. One
        // inside a turn or session also going is covered by it — a range over
        // an unfolded turn takes its rows with it, and refusing that refused
        // the whole range (Steven, 10-04). One standing alone is named, rather
        // than silently promoted to its turn, which would delete more than
        // the rows selected.
        let covered = |&(episode, index): &(EpisodeId, usize)| {
            episodes.contains(&episode)
                || turns
                    .iter()
                    .any(|t| t.episode == episode && t.index == index)
        };
        if !messages.iter().all(covered) {
            self.refuse(
                "delete acts on sessions and turns, not single messages — \
                 `h` to the turn row, or `e` to edit within it",
            );
            return None;
        }
        // A turn whose whole session is going would otherwise be counted twice
        // and rewritten into a log about to be unlinked.
        turns.retain(|t| !episodes.contains(&t.episode));

        let mut leaves_gap = false;
        for (episode, positions) in Self::positions_by_episode(&turns) {
            let Some(&earliest) = positions.first() else {
                continue;
            };
            // A gap is a surviving turn that comes AFTER something removed —
            // not merely "not the last turn". Deleting positions 0 and 2 of
            // three leaves turn 1 answering a question that is gone even though
            // the last turn went too.
            let len = graph.full_turns(&episode).len();
            leaves_gap |= (0..len).any(|p| p > earliest && !positions.contains(&p));
        }

        Some(ConfirmAction::Delete {
            episodes,
            turns,
            leaves_gap,
            claude_left: self.graph_view().map_or(0, GraphView::claude_chosen),
            // The tree the nodes were read from (`graph_nodes`: the focused
            // one), whose marks the delete spends.
            from: Some(self.workspace.focused()),
        })
    }

    /// Turn targets grouped by session, positions ascending — the shape both the
    /// gap check and the removal need, so neither re-derives it.
    pub(super) fn positions_by_episode(turns: &[TurnTarget]) -> Vec<(EpisodeId, Vec<usize>)> {
        let mut out: Vec<(EpisodeId, Vec<usize>)> = Vec::new();
        for target in turns {
            match out.iter_mut().find(|(id, _)| *id == target.episode) {
                Some((_, positions)) => positions.push(target.index),
                None => out.push((target.episode, vec![target.index])),
            }
        }
        for (_, positions) in &mut out {
            positions.sort_unstable();
            positions.dedup();
        }
        out
    }

    /// [`reset_buffer`](Self::reset_buffer) on the foreground — a test seam for
    /// "a fresh session in this buffer". No delete reaches it any more: `:delete
    /// all` resets each buffer whose session went through `reset_buffer`, as
    /// `delete_episode` does.
    #[cfg(test)]
    pub(super) fn reset_foreground(&mut self) -> Lost {
        let current = self.current;
        self.reset_buffer(current)
    }

    /// Drop prompts queued for `buffer`, and **return how many**, for the
    /// caller's sentence to name. They are the user's words; `submit` already
    /// said "queued (1)", so vanishing silently is the one thing they must not
    /// do — and the one caller, `reset_buffer`, hands the count to the delete
    /// that says what went with the session. It said its own "1 queued prompt
    /// dropped — the session they were for is gone", which named no session.
    ///
    /// Scoped to one session, because the queue is. Clearing all of it on any
    /// session change is what made "your prompt is gone" the price of pressing
    /// `:open`.
    pub(super) fn drop_queued_prompts_for(&mut self, buffer: BufferId) -> usize {
        let before = self.pending.len();
        self.pending.retain(|(b, _)| *b != buffer);
        before - self.pending.len()
    }

    /// **Every review whose draft would replace a session `gone` says is
    /// gone, dropped** — filed under another session, so its own reset does
    /// not take it (review stop 8, D 3, E1 3). How many, for the delete's
    /// sentence. The doors that delete sessions read it: `delete_episode`
    /// and `delete_all_sessions`.
    fn drop_reviews_replacing(&mut self, gone: impl Fn(EpisodeId) -> bool) -> usize {
        let replacing: Vec<BufferId> = (self.decisions.every_review())
            .filter(|(_, review)| review.work().target.replaces().is_some_and(&gone))
            .map(|(home, _)| home)
            .collect();
        (replacing.into_iter())
            .filter(|&home| self.decisions.take_review(home).is_some())
            .count()
    }

    /// **Every call awaiting a decision in `buffer`'s session, cancelled** —
    /// shown or waiting, audited as never answered and never run, its pane
    /// closed; nothing is presented after, so nothing a rule now allows
    /// starts on the way out. A typed call's record goes with it (no turn
    /// owns it to end it); a turn's goes with its turn. The count is of the
    /// typed ones: a turn's are said as "a turn in flight".
    fn cancel_decisions_of(&mut self, buffer: BufferId) -> usize {
        let shown = (self.decisions.shown(buffer))
            .map(|p| (p.tool_use_id().to_string(), p.command().to_string()));
        let waiting = self.decisions.withdraw(|w| w.buffer == buffer);
        let calls: Vec<(String, String)> = (shown.into_iter())
            .chain(waiting.into_iter().map(|w| (w.id, w.command)))
            .collect();
        self.close_decision_pane(buffer);
        let mut typed = 0;
        for (id, command) in &calls {
            self.audit("cancelled", id, command, None);
            if self.tool_generation(id).is_none() {
                typed += 1;
                self.forget_tool_call(id);
            }
        }
        typed
    }

    /// Point one buffer at a brand-new empty session, discarding what it showed.
    ///
    /// Ends the work on `buffer` and drops the prompts queued for it, then swaps
    /// in the new session. **Returns what went with the old one** — "a turn in
    /// flight", "1 queued prompt" — for the delete's sentence, the one place
    /// that says it: every caller is a delete, which names the session too.
    ///
    /// **In place**, not what `:new` does — the two shared a name and a body
    /// until `:new` became "show a fresh session in this pane"
    /// ([`new_session_command`](Self::new_session_command)). A delete cannot
    /// mint a buffer and repoint a pane at it: the pane is already showing the
    /// session that just stopped existing, and must keep its viewport and land
    /// somewhere valid. The work in flight is **discarded, not sealed** —
    /// sealing would write it back to a session the user just confirmed
    /// removing. The new session is **not written here**: like a fresh launch,
    /// it reaches disk (and becomes `HEAD`) on its first committed turn, so a
    /// fallback with no message leaves nothing behind.
    ///
    /// **It does not refill the slot it releases** — every caller loops (a
    /// session's holders, a batch `:delete`, `:delete all`); the event
    /// refills once it ends (`settle`).
    pub(super) fn reset_buffer(&mut self, buffer: BufferId) -> Lost {
        // **Every decision filed under it, first, so none is presented
        // after** (review stop 8, E1 1). Ending its work withdrew the turn's
        // calls and put the session's NEXT call to the gate — which runs it
        // when a rule now allows it: `:allow mkdir:*`, then `:delete`,
        // started a `mkdir` in a session being deleted. And a `:tool` typed
        // there, owned by no turn, stayed up, answerable in the fresh session
        // that took its place (`a` ran it). As `leaving` does: every call
        // shown or waiting there is cancelled, audited, before any work stops.
        let mut with = Lost::default();
        with.cancelled_calls(self.cancel_decisions_of(buffer));
        // The whole of "this session is going away", buffer-scoped: a
        // generation writing into it and prompts queued for it. Both calls
        // no-op when the buffer owns none of those, which is what lets every
        // delete path treat every holder the same instead of branching on
        // `current`.
        with.add(self.end_work_on(buffer, Outcome::Deleted));
        with.dropped(self.drop_queued_prompts_for(buffer));
        // **And a summary waiting for its review** (§4, design review DR-B
        // #2): a draft of a session that no longer exists composes nothing,
        // so it goes, counted in what went with it ("with a summary draft").
        // Its pane, holding nothing, closes at the event's end.
        if self.decisions.take_review(buffer).is_some() {
            with.dropped_draft();
        }
        let id = fresh_episode_id();
        let conversation = Conversation::with_episode(
            id,
            TurnConfig {
                model: self.default_model(),
                max_tokens: None,
                effort: None,
            },
        );
        let contents = Contents {
            conversation,
            output: Output::new(),
            on_disk: OnDisk::Absent, // minted just above
            billing: Billing::default(),
        };
        // Whole, through the one door, and yours: the fresh session kept
        // what you were told of the deleted one, and rang "news in" it
        // (review stop 9, A F1).
        self.hold(Some(buffer), contents, Landing::Asked);
        with
    }

    /// **Which other sessions hold a `Slice` into `id` reaching past
    /// `index`** — the sessions a cut at `index` would change. `None` when
    /// the store could not be read, which rules nothing out.
    ///
    /// **A `Slice` addresses its source by POSITION, not by content.** Delete
    /// `id` (the index-0 case) and a clone or branch — which stores only a
    /// reference, the turn contents living in `id`'s log — resolves empty on
    /// reload. Remove the turn at `index` and every later position shifts
    /// down one, so a slice with `end > index` silently yields *different
    /// turns*. [`detach_holders`](Self::detach_holders) keeps each whole.
    ///
    /// **This is not the inspector's `shared in N sessions`.** That is
    /// [`Graph::episodes_with_turn`], a count of episodes holding turns with the
    /// same content hash, and content sharing is unaffected by removing one
    /// episode's reference to it — those sessions own their own occurrence
    /// ([[feedback_content_addressed_ids]]: an id is not an occurrence).
    ///
    /// "Live" means **every open buffer**: an unpersisted clone or checkout
    /// slicing `id` is invisible to the store, and once a second buffer could
    /// exist, a foreground-only check covered a fraction of what it claimed
    /// (`[[project_shannon_n_equals_two]]`).
    pub(super) fn slice_holders(&self, id: EpisodeId, index: usize) -> Option<Vec<EpisodeId>> {
        // Segments only, so the structure: a full load here made every turn
        // delete read every body on a lazy store.
        let graph = self.store.load_structure().ok()?;
        Some(self.holders_in(&graph, id, index))
    }

    /// [`slice_holders`](Self::slice_holders) over `graph`, the store's
    /// structure as read — for a batch asking it of many sessions, which
    /// reads it once: `:delete all shannon-only` read it once per session it
    /// deleted, 3.7 s at 160 sessions (review stop 4, A2 3).
    fn holders_in(&self, graph: &Graph, id: EpisodeId, index: usize) -> Vec<EpisodeId> {
        let reaches = |ep: &Episode| {
            ep.segments().iter().any(
                |s| matches!(s, Segment::Slice { episode, end, .. } if *episode == id && *end > index),
            )
        };
        let mut holders: Vec<EpisodeId> = self
            .buffers
            .sessions()
            .map(|b| b.conversation.episode())
            .filter(|live| live.id != id && reaches(live))
            .map(|live| live.id)
            .collect();
        for e in graph.episode_ids() {
            if e != id && !holders.contains(&e) && graph.episode(&e).is_some_and(reaches) {
                holders.push(e);
            }
        }
        holders
    }

    /// **Keep every session reading from `from` past `index` whole**, before
    /// `from` loses those turns: each is detached (`Graph::detach` — what it
    /// borrows from `from` becomes its own) and written down. The sessions
    /// detached, or why not — and then the caller must not cut.
    ///
    /// A fork keeps its history when what it forked from goes, as a git
    /// branch keeps the commits another branch's deletion would otherwise
    /// take. This refused instead, naming the forks to delete first — forks
    /// folded out of sight under a turn, so "delete everything" deleted 67
    /// sessions and refused ten, four times over (Steven, 10-04).
    ///
    /// **Fails closed**: a store that cannot be read rules no reader out, and
    /// a transient error must never be the reason a cut silently rewrites
    /// someone else's history.
    pub(super) fn detach_holders(
        &mut self,
        from: EpisodeId,
        index: usize,
    ) -> Result<Vec<EpisodeId>, String> {
        let holders = self
            .slice_holders(from, index)
            .ok_or("the store could not be read to find the sessions sharing its turns")?;
        self.give_own_copies(from, &holders)?;
        Ok(holders)
    }

    /// **Each of `holders` owns what it reads from `from`**, written down
    /// (`Graph::detach`, then `write_rewritten`) — the half of
    /// [`detach_holders`](Self::detach_holders) `:delete all` takes alone,
    /// knowing its holders already. Stops at the first that cannot be
    /// written, and then the caller must not cut.
    pub(super) fn give_own_copies(
        &mut self,
        from: EpisodeId,
        holders: &[EpisodeId],
    ) -> Result<(), String> {
        if holders.is_empty() {
            return Ok(());
        }
        // Every body the copies need: what each holder derives, `from`'s
        // turns among them.
        let mut graph = self.graph_reading(holders)?;
        for &holder in holders {
            graph.detach(&holder, &from);
            // Its turns are the ones it read, where they were.
            self.write_rewritten(&graph, holder, &Some).map_err(|why| {
                format!(
                    "{} shares its turns and could not be given its own copy: {why}",
                    self.session_label(holder)
                )
            })?;
            info!(episode = %holder.to_hex(), from = %from.to_hex(), "detached a session");
        }
        Ok(())
    }

    /// **The one door a delete of chosen sessions goes through.** Two other
    /// callers remove episodes: `:delete all`, whose every reader is in its
    /// batch and goes first, so it copies only for a reader the store would
    /// not delete ([`give_own_copies`](Self::give_own_copies) — through
    /// here, it would WRITE a reader open and never written, which is going
    /// too); and a failed write's rollback in `session_ops`, which removes
    /// an episode nothing refers to yet.
    ///
    /// Both delete paths — the active session and any other by id — asked
    /// the dependents question and refused, in two places, with the same
    /// message written out twice. That is the shape that has bitten
    /// repeatedly: a guard duplicated per caller gets fixed per caller, and a
    /// mutation test aimed at one copy passes because the other is live.
    ///
    /// **Sessions sharing its turns keep them** — detached first, so nothing
    /// is orphaned — and how many is returned, for the caller to say. Gone,
    /// or why not: a batch says every failure once, beside what it did
    /// delete (Steven, 10-04: said separately, the success replaced it).
    pub(super) fn delete_from_store(&mut self, id: EpisodeId) -> Result<usize, String> {
        let kept = self.detach_holders(id, 0)?.len();
        self.store
            .delete_episode(&id)
            .map_err(|e| format!("could not delete it: {e}"))?;
        info!(episode = %id.to_hex(), kept, "deleted session");
        Ok(kept)
    }

    /// Carry out a confirmed delete: whole sessions, individual turns, or both.
    ///
    /// **Sessions first.** A turn inside a session that is also going was
    /// already dropped when the action was built, so what is left here is
    /// genuinely independent; doing sessions first also means a turn removal
    /// never rewrites a log that is about to be unlinked.
    pub(super) fn run_delete(
        &mut self,
        episodes: &[EpisodeId],
        turns: &[TurnTarget],
        from: Option<crate::wm::PaneId>,
    ) {
        // Asked again at the answer: work can start while the question is
        // up. Before the marks are spent, so they are still there to retry.
        if let Some(why) = self.rebuilt_with_work(episodes, turns) {
            self.refuse(format!("not deleted: {why}"));
            return;
        }
        // **What you were asked about is what goes**, asked here too, for the
        // same reason: a refusal must leave your marks to retry with (V4
        // verify, L3 — asked after the spend, it cleared them).
        if let Some(why) = self.turns_moved(turns) {
            self.refuse(format!("not deleted: {why} — nothing was deleted"));
            return;
        }
        let acted_on: Vec<Node> = episodes
            .iter()
            .map(|&id| Node::Episode(id))
            .chain(turns.iter().map(|t| Node::Turn {
                episode: t.episode,
                turn: t.turn,
                index: t.index,
            }))
            .collect();
        self.spend_graph_marks(from, &acted_on);
        if !episodes.is_empty() {
            self.delete_episodes(episodes);
        }
        if !turns.is_empty() {
            self.delete_turns(turns);
        }
    }

    /// **A session where a turn the question named is no longer at its
    /// place** — named — or `None`. A position names a place, and a place can
    /// come to hold another turn between the question and the answer: the
    /// answer deleted turn 3 for a question about turn 2. Nothing renumbers a
    /// session while one is up today — every change that would is work the
    /// answer refuses ([`rebuilt_with_work`](Self::rebuilt_with_work)) — so
    /// this holds it closed if one comes to. One read of the store.
    pub(super) fn turns_moved(&self, turns: &[TurnTarget]) -> Option<String> {
        if turns.is_empty() {
            return None;
        }
        let now = self.current_graph();
        Self::positions_by_episode(turns)
            .into_iter()
            .find(|(episode, _)| {
                let placed = now.placed(episode);
                (turns.iter().filter(|t| t.episode == *episode))
                    .any(|t| placed.get(t.index).map(|p| p.turn) != Some(t.turn))
            })
            .map(|(episode, _)| {
                format!(
                    "{} changed since you were asked",
                    self.session_label(episode)
                )
            })
    }

    /// **A session this delete would rebuild in place that has work running
    /// on it** — named, with what holds it and how to free it — or `None`.
    ///
    /// Rebuilt in place: the session a turn is cut from, and every session
    /// reading a deleted one's history past the cut (a clone, given its own
    /// copy by `detach_holders`) — the sessions `write_rewritten` re-adopts.
    /// Rebuilt under live work, the supersede seal appended the partial AFTER
    /// the rewrite and the buffer adopted turns computed before it: memory and
    /// disk disagreed, the partial came back mid-history, and a lazy clone was
    /// registered a second time (review 2026-10-16, Reviewer A F1, executed).
    /// Steven, 10-16: refuse while it runs. A session that is itself deleted
    /// is not rebuilt — its buffer starts fresh — so its own work ends with
    /// it, as before.
    ///
    /// Asked at the question ([`request_delete`](Self::request_delete)) and
    /// again at the answer (`run_delete`). A store that cannot be read names
    /// no reader here; `detach_holders` then refuses on its own, closed.
    pub(super) fn rebuilt_with_work(
        &self,
        episodes: &[EpisodeId],
        turns: &[TurnTarget],
    ) -> Option<String> {
        let mut rebuilt: Vec<EpisodeId> = Vec::new();
        for &e in episodes {
            rebuilt.extend(self.slice_holders(e, 0).unwrap_or_default());
        }
        for (e, positions) in Self::positions_by_episode(turns) {
            rebuilt.push(e);
            if let Some(&earliest) = positions.iter().min() {
                rebuilt.extend(self.slice_holders(e, earliest).unwrap_or_default());
            }
        }
        rebuilt.retain(|e| !episodes.contains(e));
        // **Whatever holds it**, by the one enumeration of holders: it asked
        // `conflict` alone, so a turn cut from a session whose summary draft
        // waited went through, and the draft's `a` then threw it away as
        // stale (review stop 8, A 4). Usually another session than the one
        // you are in: its CTRL-C is pressed there (§9 phase 6b).
        rebuilt.iter().find_map(|&e| {
            let (why, how) = self
                .buffers
                .sessions()
                .filter(|b| b.conversation.episode().id == e)
                .find_map(|b| self.busy_reason_and_recovery(b.id))?;
            Some(format!("{} — {why}; {how}", self.session_label(e)))
        })
    }

    /// Remove individual turns from their sessions, then persist each touched
    /// session's rewritten log.
    ///
    /// Grouped by episode and applied **highest position first**: positions name
    /// occurrences in the derived sequence, so removing index 2 renumbers
    /// everything after it and a second removal at index 5 would then land on
    /// the wrong turn. Descending order leaves every not-yet-applied index
    /// valid, which is the same reason the guard is about positions at all.
    pub(super) fn delete_turns(&mut self, turns: &[TurnTarget]) {
        let mut removed = 0usize;
        let mut kept = 0usize;
        // The sessions a turn was removed from, as the question named them —
        // and those it was not, with why.
        let mut from: Vec<String> = Vec::new();
        let mut not: Vec<String> = Vec::new();
        // That each place still holds its turn is asked at the answer, before
        // the marks are spent (`run_delete`, `turns_moved`).
        for (episode, positions) in Self::positions_by_episode(turns) {
            let count = match positions.len() {
                1 => "a turn".to_string(),
                n => format!("{n} turns"),
            };
            // Readers past the cut keep their history — found at the moment
            // of the write, not when the question was asked: a confirmation
            // can sit on screen while a background job clones this session.
            if let Some(&earliest) = positions.first() {
                match self.detach_holders(episode, earliest) {
                    Ok(holders) => kept += holders.len(),
                    Err(why) => {
                        not.push(format!(
                            "{count} of {} ({why})",
                            self.session_label(episode)
                        ));
                        continue;
                    }
                }
            }
            match self.remove_turns_from(episode, &positions) {
                Ok(()) => {
                    removed += positions.len();
                    from.push(self.session_label(episode));
                }
                Err(why) => not.push(format!(
                    "{count} of {} ({why})",
                    self.session_label(episode)
                )),
            }
        }
        // Same reasoning as the session delete: a successful removal used to
        // be the only outcome with nothing to show for it, and the graph
        // redraws either way ([[feedback_no_silent_failure]]). And named as
        // the question named it — the turn's short id, or the sessions —
        // where it said "turn deleted" (Steven, 10-16: "always say *what*
        // was deleted").
        let deleted: Vec<String> = (removed > 0)
            .then(|| {
                let what = match turns {
                    [one] if removed == 1 => format!("turn {}", one.turn.short_hex()),
                    _ if removed == 1 => "1 turn".to_string(),
                    _ => format!("{removed} turns"),
                };
                format!("{what} from {}", crate::trust::listed(&from))
            })
            .into_iter()
            .collect();
        self.say_deleted(&deleted, &not, &Self::kept_clause(kept));
        self.refresh_graph_views();
    }

    /// **The one sentence a delete ends with**, most important first (Steven,
    /// 10-16): what was deleted, what was NOT and why, then what took their
    /// place — "deleted X (with …); not deleted: Y (why) — <tail>". A refusal
    /// when anything was not deleted, and "— nothing was deleted" when
    /// nothing was; silence only when neither (the caller's own case).
    ///
    /// `delete_episodes`, `delete_turns` and `delete_all_sessions` assemble
    /// the parts and say them here: written three ways, the "not deleted"
    /// and "what took their place" came in opposite orders, and a turn
    /// delete's failure was a second refusal naming no session (review stop
    /// 2, C 7).
    pub(super) fn say_deleted(&mut self, deleted: &[String], not: &[String], tail: &str) {
        let not = (!not.is_empty()).then(|| format!("not deleted: {}", not.join("; ")));
        let deleted = (!deleted.is_empty()).then(|| crate::trust::listed(deleted));
        match (deleted, not) {
            (None, None) => {}
            (None, Some(not)) => self.refuse(format!("{not} — nothing was deleted")),
            (Some(deleted), None) => self.notify(format!("deleted {deleted}{tail}")),
            (Some(deleted), Some(not)) => self.refuse(format!("deleted {deleted}; {not}{tail}")),
        }
    }

    /// What a delete says about the sessions that shared what it removed and
    /// now own a copy — nothing when there were none.
    pub(super) fn kept_clause(kept: usize) -> String {
        match kept {
            0 => String::new(),
            1 => " — a session that shared its turns keeps its own copy".into(),
            n => format!(" — {n} sessions that shared its turns keep their own copy"),
        }
    }

    /// Apply the removal to one episode and write the result down.
    ///
    /// The in-memory graph and the store must agree, so the rewrite is built
    /// from the same `Graph` the removal was applied to and the store write is
    /// the thing whose failure is reported — a removal that only happened in
    /// memory is a turn that comes back on the next load, which reads as the
    /// delete having silently failed.
    pub(super) fn remove_turns_from(
        &mut self,
        episode: EpisodeId,
        positions: &[usize],
    ) -> Result<(), String> {
        // The bodies of what stays are rewritten and re-adopted, so loaded first.
        let mut graph = self
            .graph_reading(&[episode])
            .map_err(|why| format!("could not delete: {why}"))?;
        if !graph.remove_turns(&episode, positions) {
            return Err(format!(
                "could not delete: session {} is not in the graph",
                self.session_tag(episode)
            ));
        }
        // A kept turn moves up by the deleted ones before it; a deleted one
        // goes, and a place in it with it.
        let moved = |turn: usize| {
            (!positions.contains(&turn))
                .then(|| turn - positions.iter().filter(|&&gone| gone < turn).count())
        };
        self.write_rewritten(&graph, episode, &moved)
            .map_err(|why| format!("could not delete: {why}"))?;
        info!(episode = %episode.to_hex(), count = positions.len(), "deleted turns");
        Ok(())
    }

    /// **Write a session rebuilt in place, and show it** — the one door a
    /// rebuilt episode (`remove_turns`, `detach`) reaches the store and its
    /// buffers through, from `graph`, which holds the rebuild and its bodies.
    ///
    /// The in-memory graph and the store must agree, so the rewrite is built
    /// from the same `Graph` the change was applied to and the store write is
    /// the thing whose failure is reported — a change that only happened in
    /// memory comes back undone on the next load. Written whole, so a session
    /// not yet on disk (a lazy clone holding a slice) is registered by it.
    ///
    /// **Every window reading it stays where it was** — its place kept by
    /// position through `map` (where each turn went: a delete moves the
    /// later ones up and takes its own; a detach moves none), as every other
    /// rebuild of a session being read keeps it (`App::views_on`). A turn
    /// deleted under a window reading turn 8 sent it to the end (review stop
    /// 15, A L8).
    pub(super) fn write_rewritten(
        &mut self,
        graph: &Graph,
        episode: EpisodeId,
        map: super::marks::TurnMap<'_>,
    ) -> Result<(), String> {
        let Some(rewritten) = graph.episode(&episode).cloned() else {
            return Err("the session vanished mid-edit".into());
        };
        // Every own turn's content has to travel with the new log, or the next
        // load silently produces a shorter session — one record per occurrence,
        // so each keeps its own facts.
        let mut own: Vec<shannon_core::graph::Turn> = Vec::new();
        for occurrence in rewritten.own_occurrences() {
            let Some(messages) = graph.messages(&occurrence.turn) else {
                return Err(format!(
                    "turn {} has no content in the graph",
                    occurrence.turn.short_hex()
                ));
            };
            own.push(Turn::of(occurrence, messages.to_vec()));
        }
        self.store
            .rewrite_episode(&rewritten, &own)
            .map_err(|e| format!("could not write it: {e}"))?;

        // **Every buffer showing this session, through the same door.** A live
        // buffer holds its own `Conversation`, so a pane on this session would
        // go on rendering the turn that is now off disk — the shape that made a
        // delete read as a no-op before. `adopt_session` is how a buffer takes
        // on a different history everywhere else (clone, checkout, squash), and
        // a second way of doing it here is how the two would drift.
        //
        // On disk with every OWN turn: unlike a lazy clone, this episode's own
        // turns are written already — we just wrote them — so claiming fewer
        // would have the next turn re-append them. Own, not the derived length:
        // a branch's shared prefix is not its to count. This passed the full
        // length once, which on a branch put the cursor past the end and left
        // every later turn in memory only.
        let turns: Vec<shannon_core::graph::Turn> = graph
            .records(&episode)
            .ok_or("written, but part of the session is not in the store to show")?;
        let holders: Vec<BufferId> = self
            .buffers
            .sessions()
            .filter(|b| b.conversation.episode().id == episode)
            .map(|b| b.id)
            .collect();
        // Each holder shows this session, so its secrets were unlocked when
        // it opened; a store that refuses now leaves that holder as it was.
        for buffer in holders {
            let recall = self.unlock_turns(&turns)?;
            let views = self.views_on(buffer, map);
            // Yours: a turn deleted, or a reader detached by it.
            self.adopt_session(
                buffer,
                rewritten.clone(),
                turns.clone(),
                OnDisk::Own(rewritten.own_len()),
                recall,
                Landing::Asked,
            );
            self.put_views(buffer, views);
        }
        Ok(())
    }

    /// Delete a session by id (`:delete`, `:delete <hash>`, `d` in `:graph`).
    ///
    /// **Every buffer that was showing it starts fresh in place.** A pane must
    /// not render a session that is gone, and it must not vanish either — it
    /// falls back to the same empty session a fresh start gives you. That used
    /// to be a special case for "the active session"; it is now simply what
    /// happens to each holder, because there can be more than one and a rule
    /// written for exactly one of them is a rule that covers a fraction of the
    /// cases it claims.
    pub(super) fn delete_episodes(&mut self, ids: &[EpisodeId]) {
        // Each deleted session as the report names it, with what went with it.
        let mut gone: Vec<(EpisodeId, String)> = Vec::new();
        let mut replaced = 0usize;
        let mut copies = 0usize;
        let mut refused: Vec<(EpisodeId, String)> = Vec::new();
        for id in self.forks_first(ids) {
            // Named BEFORE it goes: afterwards its `:ls` number names the
            // fresh session that took its place, and its name names nothing.
            let name = self.session_label(id);
            // Counted from what the delete REPORTS, never by probing the
            // store afterwards: an unpersisted live session is absent from
            // the store both before and after, so "is it gone now" cannot
            // tell a refusal from a session that was never on disk.
            match self.delete_episode(id) {
                Deleted::Refused(why) => refused.push((id, why)),
                Deleted::Removed {
                    replaced: r,
                    kept: k,
                    with,
                } => {
                    gone.push((id, format!("{name}{}", with.clause())));
                    replaced += usize::from(r);
                    copies += k;
                }
            }
        }
        // In the order they were asked about, not the order they went in
        // (`forks_first`), so the report reads like the question.
        let named: Vec<String> = ids
            .iter()
            .filter_map(|id| gone.iter().find(|(g, _)| g == id))
            .map(|(_, name)| name.clone())
            .collect();
        // **What was NOT deleted, in the same sentence as what was.** Said
        // separately, the success notice replaced the refusal, and the kept
        // session sat there still marked with nothing to say why (Steven,
        // 10-04).
        let not: Vec<String> = refused
            .iter()
            .map(|(id, why)| format!("{} ({why})", self.session_label(*id)))
            .collect();
        // **Say that it worked.** Every message on this path was a failure
        // message, so a successful delete said nothing at all — and deleting the
        // LIVE session immediately mints a fresh one in its place, which
        // reappears as a row. The screen therefore contradicted the action:
        // Steven deleted the top row repeatedly, watched a row stay there, and
        // read it as a loop where the delete "did nothing". Silence about
        // success is as damaging as silence about failure when the visible
        // evidence disagrees ([[feedback_no_silent_failure]]).
        //
        // **And say WHAT** (Steven, 10-16: "We must always say *what* was
        // deleted"): each session as the question named it, and what went with
        // it — "deleted b3 fix-parser (with a turn in flight and 1 queued
        // prompt)". It said "session deleted" after asking "delete session b3
        // fix-parser?", and the work it ended spoke for itself or not at all:
        // a distillation said its session "changed", a turn, a job and a
        // continuation said nothing, the queue said "the session they were for
        // is gone", naming none.
        //
        // Naming the replacement is the whole point: it is why a row is
        // still on screen. Said of the BUFFER, which is what was
        // replaced: "this pane" was wrong whenever the deleted session
        // was hidden or shown elsewhere (Reviewer F, round 16). Counted, now
        // that the sentence names several: one "took its place" for two.
        let tail = match replaced {
            0 => "",
            1 => " — a fresh empty session took its place in its buffer",
            _ => " — fresh empty sessions took their places in their buffers",
        };
        let tail = format!("{tail}{}", Self::kept_clause(copies));
        self.say_deleted(&named, &not, &tail);
        self.refresh_graph_views();
        // The slots this released refill when the event settles (`settle`),
        // after the WHOLE `:delete` — `run_delete` runs `delete_turns` after
        // this tail.
    }

    /// Delete one session, reporting which of the two things happened — see
    /// [`Deleted`].
    pub(super) fn delete_episode(&mut self, id: EpisodeId) -> Deleted {
        let kept = match self.delete_from_store(id) {
            Ok(kept) => kept,
            // Failed: nothing to fall back from.
            Err(why) => return Deleted::Refused(why),
        };
        let holders: Vec<BufferId> = self
            .buffers
            .sessions()
            .filter(|b| b.conversation.episode().id == id)
            .map(|b| b.id)
            .collect();
        let replaced = !holders.is_empty();
        // **Every holder, identically.** The `if buffer == self.current` branch
        // that used to be here was `n = 2` written as a branch: only the
        // foreground arm discarded the turn, the decision and the queued
        // prompts. Since a generation can own a buffer that is not `current` —
        // the headline P4 workflow — deleting *that* session took the other
        // arm, and the permission gate for a deleted session stayed on screen
        // and answerable. Pressing `a` ran the command
        // ([[project_shannon_n_equals_two]]).
        //
        // It does NOT repoint `HEAD`: nothing on this path writes a ref, and a
        // resume from a deleted episode falls back to a fresh session on
        // `EpisodeNotFound`.
        let mut with = Lost::default();
        for buffer in holders {
            with.add(self.reset_buffer(buffer));
        }
        // **And a review that would replace it**, filed under another
        // session (`S` on a branch files its review under the session it was
        // pressed in): its draft summarises a session that is gone, so it
        // goes with it, counted, as one filed under the session itself does
        // (`reset_buffer`). It waited on, said nothing, and its `a` failed
        // later with the merge's own "episode not found" (review stop 8, D 3).
        // The other half of `review_holding`'s pair: what holds the session
        // goes with it.
        for _ in 0..self.drop_reviews_replacing(|e| e == id) {
            with.dropped_draft();
        }
        // The viewer rebuild is the CALLER's, once, after the whole batch —
        // refreshing per id would rebuild N times and re-land the cursor N
        // times for one keypress.
        Deleted::Removed {
            replaced,
            kept,
            with,
        }
    }

    /// **`ids` with every session before the ones it reads from** — so a fork
    /// deleted in the same batch goes first, and is never given a copy of
    /// its history only to be deleted a moment later. Order otherwise as
    /// given. One snapshot for the whole batch; slices form no cycle (a
    /// session reads only from what existed before it), and anything left
    /// over goes last regardless.
    pub(super) fn forks_first(&self, ids: &[EpisodeId]) -> Vec<EpisodeId> {
        let graph = self.current_graph();
        let reads_from = |reader: &EpisodeId, source: &EpisodeId| {
            graph.episode(reader).is_some_and(|ep| {
                ep.segments()
                    .iter()
                    .any(|s| matches!(s, Segment::Slice { episode, .. } if episode == source))
            })
        };
        let mut left: Vec<EpisodeId> = ids.to_vec();
        let mut order: Vec<EpisodeId> = Vec::with_capacity(ids.len());
        while !left.is_empty() {
            let (ready, waiting): (Vec<EpisodeId>, Vec<EpisodeId>) = left.iter().partition(|id| {
                !left
                    .iter()
                    .any(|other| other != *id && reads_from(other, id))
            });
            if ready.is_empty() {
                order.extend(waiting);
                break;
            }
            order.extend(ready);
            left = waiting;
        }
        order
    }

    /// Either gate a destructive action behind a `y`/`n` confirmation (the
    /// default) or run it immediately when `tui.confirm_delete = false`. The
    /// single chokepoint for every delete entrypoint.
    pub(super) fn request_delete(&mut self, action: ConfirmAction) {
        // Refused before it is asked, not after a `y`: a delete that would
        // rebuild a session with work running on it (`rebuilt_with_work`).
        if let ConfirmAction::Delete {
            episodes, turns, ..
        } = &action
            && let Some(why) = self.rebuilt_with_work(episodes, turns)
        {
            return self.refuse(format!("not deleted: {why}"));
        }
        if self.config.tui.confirm_delete {
            self.ask(action);
        } else {
            self.run_confirmed(action);
        }
    }

    /// **The sessions going on from Claude Code conversations**, stored or
    /// open and never written: an import's record (`Episode::inherited`), or
    /// the look of an import made before imports recorded one, which no match
    /// found (`reads_imported`, no origin) — "possibly inherited" (Steven,
    /// 10-16). What `:delete all` asks a second time about, and `:delete all
    /// shannon-only` keeps. A clone or branch of one was born here, and goes:
    /// the inspector says so, by the same answer (`import_flow::from_claude`).
    pub(super) fn continued_sessions(&self) -> Vec<EpisodeId> {
        use super::import_flow::{FromClaude, from_claude};
        let graph = self.current_graph();
        graph
            .episode_ids()
            .into_iter()
            .filter(|id| from_claude(&graph, id).is_some_and(FromClaude::continues))
            .collect()
    }

    /// Prune **every** session from the store but `keep`, and start every
    /// buffer whose session went on a fresh one. Reached only via a confirmed
    /// `:delete all` (`keep` empty), or `:delete all shannon-only` (`keep`
    /// the sessions going on from Claude Code — kept as a refused one is:
    /// what reads from a session that goes keeps its own copy, its buffer
    /// stays).
    ///
    /// **A buffer is reset only for a session that is gone** (Steven, 10-16:
    /// "we shouldn't refresh in the setting where we fail to delete. This will
    /// likely confuse the user to making them believe the action was
    /// successful when it wasn't"). A store that cannot be listed resets
    /// nothing; one that refuses every delete resets nothing; one that refuses
    /// some keeps the buffers showing those, and says which. Each reset every
    /// buffer, and ended the work in it, whatever the store had done.
    pub(super) fn delete_all_sessions(&mut self, keep: &[EpisodeId]) {
        let ids = match self.store.list_episodes() {
            Ok(ids) => ids,
            Err(e) => {
                return self.refuse(format!(
                    "could not list sessions to delete: {e} — nothing deleted"
                ));
            }
        };
        let count = ids.iter().filter(|id| !keep.contains(id)).count();
        // Sessions open and never written are deleted too — their buffer is
        // their only copy — and named, as what was deleted is.
        let never: Vec<String> = self
            .buffers
            .sessions()
            .map(|b| b.conversation.episode())
            .filter(|e| !e.segments().is_empty() && !ids.contains(&e.id) && !keep.contains(&e.id))
            .map(|e| self.session_label(e.id))
            .collect();
        // What stays: what was asked to, and what the store refused.
        let mut kept: Vec<EpisodeId> = keep.to_vec();
        let mut why_kept: Vec<String> = Vec::new();
        // The store's structure, read once for the readers of every session
        // going — and again only after readers were given their own copies,
        // whose slices are then not what was read: one detached from X can
        // come to slice what X read (`Graph::detach`).
        let mut structure: Option<Graph> = None;
        for id in self.forks_first(&ids) {
            if keep.contains(&id) {
                continue;
            }
            // **Forks first**, so every session reading `id` has had its
            // turn: gone, or KEPT by a refusal — and a kept one keeps what it
            // reads, its own copy made before `id` goes, as every other delete
            // gives it (review stop 2, A F4 · B 1 · C 9: its buffer stayed
            // whole while its log came back short). Only the kept: a reader
            // open and never written is going too, and a copy would write it.
            if !kept.is_empty() && structure.is_none() {
                structure = self.store.load_structure().ok();
            }
            let readers = match (kept.is_empty(), &structure) {
                (true, _) => Ok(Vec::new()),
                (false, Some(graph)) => Ok(self
                    .holders_in(graph, id, 0)
                    .into_iter()
                    .filter(|h| kept.contains(h))
                    .collect()),
                (false, None) => Err(
                    "the store could not be read to find the sessions sharing its turns"
                        .to_string(),
                ),
            };
            let gone = readers
                .and_then(|readers: Vec<EpisodeId>| {
                    if !readers.is_empty() {
                        structure = None;
                    }
                    self.give_own_copies(id, &readers)
                })
                .and_then(|()| self.store.delete_episode(&id).map_err(|e| e.to_string()));
            if let Err(e) = gone {
                warn!(episode = %id.to_hex(), error = %e, "could not delete episode during prune");
                why_kept.push(format!("{} ({e})", self.session_label(id)));
                kept.push(id);
            }
        }
        let refused = kept.len() - keep.len();
        info!(
            count,
            failures = refused,
            kept = keep.len(),
            "pruned all sessions"
        );
        let deleted = count - refused;
        // What it kept, by name, as what it deleted is (`[[feedback_say_
        // what_was_deleted]]`) — in every sentence it ends with: two left it
        // out, and one said there was nothing stored (review stop 4, A2 7).
        let kept_said = (!keep.is_empty()).then(|| {
            let names: Vec<String> = keep.iter().map(|id| self.session_label(*id)).collect();
            format!(
                "kept {} going on from Claude Code ({})",
                crate::view::counted(keep.len(), "session", "sessions"),
                crate::trust::listed(&names)
            )
        });
        if count > 0 && deleted == 0 {
            let not: Vec<String> = why_kept.into_iter().chain(kept_said).collect();
            return self.say_deleted(&[], &not, "");
        }
        // **Every** buffer whose session went, not just the foreground one.
        // `delete_episode` is deliberately written over every holder; this
        // path predates that and reset only the session in front, leaving a
        // background pane rendering something that is gone from disk — and,
        // if that buffer held a lazy clone, holding `Slice`s into deleted logs
        // that would persist as an unresolvable episode on its next turn.
        let going: Vec<BufferId> = (self.buffers.sessions())
            .filter(|b| !kept.contains(&b.conversation.episode().id))
            .map(|b| b.id)
            .collect();
        // Sessions, counted as such: a page or a tree is a buffer too, and
        // "every buffer" said of sessions turned false with them (V5.4
        // design review, L7).
        let some_stay = going.len() < self.buffers.sessions().count();
        // Across every buffer, counted together: "2 turns in flight", not a
        // turn per buffer said again (`Lost`).
        let mut lost = Lost::default();
        for buffer in going {
            lost.add(self.reset_buffer(buffer));
        }
        // And a review kept that would replace one gone — `delete_episode`'s
        // rule, at this door too: the branch it folds went, unsaid, and only
        // `d` could end it (review stop 8, E1 3).
        for _ in 0..self.drop_reviews_replacing(|e| !kept.contains(&e)) {
            lost.dropped_draft();
        }
        let with = lost.clause();
        // **Say that it worked**, for the reason `delete_episodes` gives: the
        // reset mints a fresh session that reappears as a row, so a silent
        // success is contradicted by the screen and reads as "it did nothing".
        // That fix landed on the instance that was reported and not on the
        // class, and the member it missed is the most destructive command in
        // the product ([[feedback_class_not_instance]]). **And say what** —
        // its count, the sessions never written, what went with them, and what
        // was NOT deleted — in `delete_episodes`' shape (Steven, 10-16).
        let whose = match keep.is_empty() {
            true => "stored",
            false => "born-in-Shannon",
        };
        let stored = match (count, refused) {
            (0, _) => None,
            (1, 0) => Some(format!("the one {whose} session")),
            (n, 0) => Some(format!("all {n} {whose} sessions")),
            (n, _) => Some(format!("{deleted} of {n} {whose} sessions")),
        };
        let never =
            (!never.is_empty()).then(|| format!("{}, never written", crate::trust::listed(&never)));
        let what: Vec<String> = stored.into_iter().chain(never).collect();
        let mut reset = match some_stay {
            false => "every open session is now a fresh empty one".to_string(),
            true => "every open session but theirs is now a fresh empty one".to_string(),
        };
        if let Some(kept) = &kept_said {
            reset = format!("{kept}; {reset}");
        }
        match what.as_slice() {
            // Nothing on disk is not a failure, but it is not silence either:
            // `:delete all` that says nothing is indistinguishable from
            // `:delete all` that did not run. The reset is said only when it
            // ended something — there being nothing deleted to hang it on.
            [] if with.is_empty() => self.notify(match &kept_said {
                None => format!("no {whose} sessions to delete"),
                Some(kept) => format!("no {whose} sessions to delete; {kept}"),
            }),
            [] => self.notify(format!("no {whose} sessions to delete; {reset}{with}")),
            // What went with them, said of the whole: one reset, many buffers.
            what => self.say_deleted(
                &[format!("{}{with}", crate::trust::listed(what))],
                &why_kept,
                &format!(" — {reset}"),
            ),
        }
        // Every buffer reset dropped the prompts queued for it, said in the
        // sentence above — the event's settle finds nothing left to start
        // (`a_batch_delete_starts_nothing_it_is_about_to_delete`).
    }
}
