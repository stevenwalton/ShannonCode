//! **Writing sessions to the store** — the one persist, the on-disk
//! cursor it keeps per buffer (`OnDisk`), what a session owes, `HEAD`'s
//! rule, and `:w` / `:wa`. Moved out of `app.rs` whole (review stop 2,
//! D7: "We shouldn't let files get too big").

use super::*;

impl App {
    /// Append any not-yet-persisted turns to the store — usually exactly the one
    /// just committed; more only if a prior write failed (this backfills the
    /// gap). Best-effort: a write failure is logged and the in-memory transcript
    /// is kept (losing durability beats losing the user's turn), leaving the rest
    /// for the next commit to retry.
    ///
    /// Persistence is **positional**, not keyed by `TurnId`: ids are
    /// content-addressed, so two identical consecutive turns share an id —
    /// id-dedup would silently drop the legitimate repeat (data loss on resume).
    /// **Test-only now**, and that is the interesting part: no production path
    /// persists "the current buffer" any more. A turn seals into the buffer its
    /// generation owns, so every caller names one.
    #[cfg(test)]
    pub(super) fn persist_committed_turn(&mut self) {
        self.persist_saying(self.current, None);
    }

    /// **Write what `buffer`'s session owes the store**, saying a failure —
    /// "NOT SAVED TO DISK", in one wording — and making a session written for
    /// the FIRST time `HEAD` by the one rule, [`moves_head`](Self::moves_head).
    /// Every write a person or their work causes comes here: a turn's commit
    /// (into the buffer it was born on, which may no longer be in front), an
    /// interrupted turn's seal, `:w`, `:wa`, `:fork`'s source, a
    /// composition's sources and its result, an import, a carried name. Jobs
    /// write through [`persist_buffer`](Self::persist_buffer) and never move
    /// `HEAD` (a child's session is not one a launch returns to).
    ///
    /// Returns **what happened**, for `:write`. The automatic callers ignore
    /// it: they run after a commit, where the only outcome worth a word is a
    /// failure — which this reports itself, because a persist that fails
    /// silently is a session the user believes is on disk. `:write` is the
    /// one caller that must also say "done" and "nothing to do", since a verb
    /// that answers only on failure is a dead key on the happy path.
    pub(super) fn persist_saying(
        &mut self,
        buffer: BufferId,
        replaces: Option<EpisodeId>,
    ) -> Saved {
        let head = self.moves_head(buffer, replaces);
        match self.persist_buffer(buffer, head) {
            Ok(saved) => saved,
            Err(why) => {
                // Some turns may have landed before the failure. The refusal is
                // the thing that matters; a count beside it would bury it.
                //
                // Named off the aim (`refuse_for`): a turn ending in a pane you
                // left, an unreviewed summary, a composed continuation write
                // here, and the one notice that most needs to say whose said
                // "this session" of one you were not in (full review 10-21,
                // E 5).
                self.refuse_for(buffer, format!("NOT SAVED TO DISK — {why}"));
                Saved::Refused
            }
        }
    }

    /// **Does a first write of `buffer`'s session make it `HEAD`** — where a
    /// `resume = last` launch opens? When it is **where you are**, the
    /// session your next message goes to (Steven, 10-16); or when it
    /// REPLACES the session `HEAD` names (`replaces`: a compaction's
    /// `carries`, a carried name), as a branch ref follows a rebase — a
    /// launch must not reopen an original that handed its name and its work
    /// on. So `:compact` reopens on the compacted session after a quit, as
    /// Claude Code's `/compact` then `claude --continue` does, while
    /// anything finishing in a pane you have left — a turn, a summary, a
    /// `:wa` of a clone beside you — does not move where the next launch
    /// opens ("If we put a pane in the background I don't want to teleport
    /// to it"). It said "always" for six callers and asked this for two, and
    /// `:wa`, the quit refusal's remedy, made a background clone `HEAD`
    /// (review stop 2, A F3 · C 4, D1).
    pub(super) fn moves_head(&self, buffer: BufferId, replaces: Option<EpisodeId>) -> bool {
        let here = buffer == self.prompt_target();
        here || replaces.is_some_and(
            |from| matches!(self.store.get_ref("HEAD"), Ok(Some(head)) if head == from),
        )
    }

    /// **Make `id` the session a `resume = last` launch opens** — said, if it
    /// cannot be recorded: the session in hand is fine, but the next launch
    /// opens another with no reason given. The one wording for every writer
    /// of `HEAD` in a running app (a first write, `:open`, a branch edit);
    /// two said it differently and the third only in the log.
    pub(super) fn set_head(&mut self, id: EpisodeId) {
        if let Err(e) = self.store.put_ref("HEAD", &id) {
            warn!(episode = %id.to_hex(), error = %e, "could not record HEAD");
            self.refuse(format!(
                "could not record {} as the session to reopen ({e}) — the next launch \
                 may open another",
                self.session_label(id)
            ));
        }
    }

    /// **A session just made that owns new content is written at once**
    /// (Steven, 10-16, decision (a)) — a squash's summary, an import's turns,
    /// a continuation composed over the turn it extended: text nothing else
    /// holds, which a lazy session lost to `:q!` or a crash. One that only
    /// shares another's turns — a clone, a checkout, a straight merge, a
    /// prune — stays lazy, written on its first turn or `:w`, and named at
    /// `:q` until then ([`unwritten`](Self::unwritten)). Through
    /// [`persist_saying`](Self::persist_saying): a write that fails says "NOT
    /// SAVED TO DISK", and `HEAD` moves by its rule
    /// ([`moves_head`](Self::moves_head)).
    ///
    /// Called where a making ENDS — `adopt_composed`, for every composition,
    /// after it carries a name; `import_conversation` — and not in
    /// `adopt_session`, which a carried name follows: written there, the
    /// carry's own write found nothing pending and the name never reached
    /// the header. (A branch edit that owns turns writes before it adopts,
    /// on its own path, and aborts when that fails.)
    pub(super) fn write_if_it_owns(&mut self, buffer: BufferId, replaces: Option<EpisodeId>) {
        let owns = self
            .session_of(buffer)
            .is_some_and(|b| b.conversation.episode().own_len() > 0);
        if owns {
            self.persist_saying(buffer, replaces);
        }
    }

    /// **The one persist**: write what `buffer`'s session owes the store, per
    /// its [`OnDisk`] cursor. The foreground path and every background job end
    /// here — they had two copies with two rules for "never written", one a
    /// proxy and one asking the store, and the proxy was the one that shipped
    /// bugs. `head` says whether a first write also makes the session `HEAD`,
    /// where a `resume = last` launch returns: a foreground session's, never a
    /// child's.
    ///
    /// The error is the reason, worded to follow "NOT SAVED TO DISK — "; each
    /// caller says it its own way (the foreground on the notice line, a job in
    /// its "could NOT be saved" accounting) — and the buffer keeps it
    /// ([`SessionBuffer::write_failed`]) until a write succeeds, for `:q` to
    /// name.
    pub(super) fn persist_buffer(&mut self, buffer: BufferId, head: bool) -> Result<Saved, String> {
        let wrote = self.write_owed(buffer, head);
        if let Some(b) = self.session_of_mut(buffer) {
            b.write_failed = wrote.as_ref().err().cloned();
        }
        wrote
    }

    /// [`persist_buffer`](Self::persist_buffer)'s write, without the record of
    /// how it went.
    pub(super) fn write_owed(&mut self, buffer: BufferId, head: bool) -> Result<Saved, String> {
        let Some(target) = self.session_of(buffer) else {
            return Ok(Saved::Gone); // the buffer closed under the turn
        };
        let episode = target.conversation.episode().clone();
        let written = self.written_of(target)?;
        self.set_on_disk(buffer, written.map_or(OnDisk::Absent, OnDisk::Own));
        if !Self::owes(written, &episode) {
            // A session with no history never touches disk, so a fresh one
            // becomes `HEAD` only once it has content.
            return Ok(match written {
                None => Saved::Empty,
                Some(_) => Saved::UpToDate,
            });
        }
        match written {
            // Register: replay the whole segment structure atomically — slices
            // reference shared history, own turns carry new content, so a shared
            // prefix is a small record, never a copy, and a partial write is
            // rolled back rather than left to resolve wrong.
            None => {
                let own: Vec<Turn> = self
                    .session_of(buffer)
                    .map(|b| b.conversation.own_turns())
                    .unwrap_or_default();
                self.persist_episode(&episode, &own)
                    .map_err(|e| format!("this session is in memory only: {e}"))?;
                self.set_on_disk(buffer, OnDisk::Own(episode.own_len()));
                if head {
                    self.set_head(episode.id);
                }
                Ok(Saved::Wrote(episode.own_len()))
            }
            // Registered: append the own turns past the cursor, moving it
            // with each one, so a failure part-way leaves it true.
            Some(written) => {
                let pending: Vec<Turn> = self
                    .session_of(buffer)
                    .map(|b| b.conversation.own_turns_from(written))
                    .unwrap_or_default();
                for (i, turn) in pending.iter().enumerate() {
                    self.store
                        .append_turn(&episode.id, turn)
                        .map_err(|e| format!("this turn is in memory only: {e}"))?;
                    self.set_on_disk(buffer, OnDisk::Own(written + i + 1));
                }
                Ok(Saved::Wrote(pending.len()))
            }
        }
    }

    /// **What the store holds of `buffer`'s session** — `None` when it is not
    /// registered, else how many of its own turns are written: its [`OnDisk`]
    /// cursor, resolved. The one reading of the cursor, for the write that
    /// pays what is owed ([`persist_buffer`](Self::persist_buffer)) and the
    /// quit that must not leave it unpaid ([`unwritten`](Self::unwritten)).
    ///
    /// **Only a known absence is absence.** An unknown cursor asks the store; a
    /// store that cannot say is an error, never "absent" — registering a
    /// session that IS on disk appends a second header, and the registration's
    /// rollback deletes the log (Reviewer E, round 16).
    pub(super) fn written_of(&self, buffer: &SessionBuffer) -> Result<Option<usize>, String> {
        match buffer.on_disk {
            OnDisk::Unknown => self
                .store
                .episode_len(&buffer.conversation.episode().id)
                .map_err(|e| format!("could not tell what of this session is on disk: {e}")),
            OnDisk::Absent => Ok(None),
            OnDisk::Own(n) => Ok(Some(n)),
        }
    }

    /// **Does the store owe `episode` anything**, holding `written` of it
    /// ([`written_of`](Self::written_of))? An unregistered session with any
    /// history — a lazy clone shares every turn and owns none, and is still a
    /// session to lose — or a registered one with own turns past the cursor.
    pub(super) fn owes(written: Option<usize>, episode: &Episode) -> bool {
        match written {
            None => !episode.segments().is_empty(),
            Some(n) => episode.own_len() > n,
        }
    }

    /// **The open sessions the store does not hold whole**, as `:ls` names
    /// them — and those it cannot say about, with why. What `:q` refuses over
    /// without `!` (Steven, 10-16: "any 'unwritten session' and refusing
    /// without a force"): a lazy one (a clone, a checkout, a straight merge,
    /// a prune — written on its first turn or `:w`) and one whose write
    /// failed. Read through the write's own reading of the cursor, so the
    /// guard and the write cannot disagree about what is owed. Each owed one
    /// with why its last write failed, if it did.
    ///
    /// **But a look is not a loss** (S21-Q12, Steven 11-01: agreed): a
    /// session never written counts only if it holds something of its own
    /// (`SessionBuffer::has_its_own`). A checkout or a clone only looked at
    /// still owes `:w` its write — you can keep one — but a quit loses
    /// nothing of it, as the manual says ("leaving keeps nothing"), and the
    /// refusal's own answer, `:wa`, wrote a branch nobody made (review stop
    /// 15, F H3, measured).
    pub(super) fn unwritten(&self) -> Unwritten {
        let (mut owed, mut unsure) = (Vec::new(), Vec::new());
        for b in self.buffers.sessions() {
            let episode = b.conversation.episode();
            // An empty session owes nothing, whatever the store would say —
            // asked first, so the fresh session a launch starts on (its
            // cursor `Unknown`) is never "perhaps unwritten" over a store that
            // cannot be read.
            if episode.segments().is_empty() {
                continue;
            }
            match self.written_of(b) {
                // A registered one owes only turns of its own, so the second
                // term speaks for one never written.
                Ok(written) if Self::owes(written, episode) && b.has_its_own() => {
                    owed.push((self.session_label(episode.id), b.write_failed.clone()));
                }
                Ok(_) => {}
                Err(why) => unsure.push((self.session_label(episode.id), why)),
            }
        }
        Unwritten { owed, unsure }
    }

    /// Move `buffer`'s persist cursor — a no-op once the buffer is gone.
    pub(super) fn set_on_disk(&mut self, buffer: BufferId, state: OnDisk) {
        if let Some(b) = self.session_of_mut(buffer) {
            b.on_disk = state;
        }
    }

    /// `:write` (`:w`) — **flush this session to the store now.**
    ///
    /// Turns persist as they commit, so the usual answer is "already on disk" —
    /// and that is worth having. A vim user types `:w` reflexively, and an
    /// answer confirms the auto-persist model instead of leaving them to guess
    /// at it. The case it exists FOR is the other one: when a persist failed,
    /// the session lives in memory and the message naming that state had no
    /// verb to offer (Reviewer F). Now it has one, and it is vim's.
    ///
    /// Deliberately the same door the automatic path uses, not a second
    /// persist: a recovery that writes through different code than the thing it
    /// is recovering is a recovery for a different bug
    /// (`[[feedback_one_door_per_invariant]]`).
    ///
    /// Returns whether the session **is on disk now** — which is what `:wq`
    /// needs in order to decide whether quitting would discard anything. A
    /// caller that reads the notice line instead would be reading a proxy: the
    /// message is for the human, and inferring control flow from its wording
    /// puts a behaviour one careless rephrase away from inverting.
    ///
    /// **`:w <name>` names the session as it writes it** — vim's `cpo-F`
    /// (Steven, 10-14: "`:w foo` for vim users"): one with no name takes it,
    /// one already so named just writes, and one named otherwise is refused
    /// toward `:rename` ([`name_for_write`](Self::name_for_write)). The name is
    /// a session's name, never a file.
    ///
    /// **"This session" is the aim**, as for every session command
    /// ([`target_conversation`](Self::target_conversation)) and as the status
    /// row says. It read `focused_buffer`, which agrees everywhere but in a
    /// viewer LENT over another session's transcript: there the row said the
    /// aim and `:w` saved the session beneath (10-16).
    pub(crate) fn write_command(&mut self, args: &str) -> bool {
        // One read for what is saved AND what is said, so the two cannot
        // name different sessions.
        let Some((buffer, episode)) =
            (self.aimed_session()).map(|b| (b.id, b.conversation.episode().id))
        else {
            let why = self.no_agent();
            self.refuse(format!("`:write`: {why}"));
            return false;
        };
        let named = match args.split_whitespace().collect::<Vec<_>>().as_slice() {
            [] => None,
            [name] => match self.name_for_write(buffer, name) {
                Some(true) => Some(name.to_string()),
                Some(false) => None,
                None => return false,
            },
            _ => {
                self.refuse("`:write` takes a session name, one word — it writes no file");
                return false;
            }
        };
        // The session the way every other message names one — and a name it
        // was given just now said first, so `:w notes.md` meant as an export
        // reads at once as what it did.
        let tag = self.session_tag(episode);
        let label = match &named {
            Some(name) => format!("named `{name}` · {tag}"),
            None => tag,
        };
        match self.persist_saying(buffer, None) {
            Saved::UpToDate => {
                self.notify(format!("{label}: already on disk"));
                true
            }
            // Nothing to keep, so quitting discards nothing — `true` for `:wq`,
            // but not the claim that it is on disk.
            // The name lives with the session and is written with its first
            // turn — so `:wq` here drops both, and says nothing it cannot keep.
            Saved::Empty if named.is_some() => {
                self.notify(format!(
                    "{label}: nothing to save yet — the name is written with its first turn"
                ));
                true
            }
            Saved::Empty => {
                self.notify(format!("{label}: nothing to save — the session is empty"));
                true
            }
            // A session that shares every turn it shows (a clone, a merge, a
            // prune) is written with none of its own: say what happened.
            Saved::Wrote(0) => {
                self.notify(format!("{label}: saved (its turns are shared history)"));
                true
            }
            Saved::Wrote(n) => {
                self.notify(format!("{label}: {n} turn(s) written"));
                true
            }
            // Both already said their piece — `Refused` on the notice line,
            // `Gone` never from here.
            Saved::Refused => false,
            Saved::Gone => {
                self.refuse("that session is no longer open");
                false
            }
        }
    }

    /// `:wall` (`:wa`) — flush **every** open session.
    ///
    /// Earned by the multi-session workflow: `:Send` to three panes with a disk
    /// problem leaves three sessions in memory, and making the user `:open` each
    /// one to save it is a recovery worse than the failure. vim's verb, vim's
    /// scope.
    pub(crate) fn write_all_command(&mut self) {
        let buffers: Vec<BufferId> = self.buffers.sessions().map(|b| b.id).collect();
        let (mut written, mut sessions, mut failed) = (0usize, 0usize, 0usize);
        // Each by `HEAD`'s one rule: only where you are moves it — `:wa`,
        // the quit refusal's remedy, made a background clone `HEAD` (D1).
        for buffer in buffers {
            match self.persist_saying(buffer, None) {
                Saved::Wrote(n) => {
                    written += n;
                    sessions += 1;
                }
                Saved::Refused | Saved::Gone => failed += 1,
                Saved::UpToDate | Saved::Empty => {}
            }
        }
        // A failure has already put its own reason on the notice line, and this
        // must not overwrite it with a cheerier summary — so when anything
        // failed, the only thing added is the count that the reason cannot
        // convey (how many others are in the same state).
        if failed > 0 {
            self.refuse(format!("{failed} session(s) NOT saved — see :messages"));
        } else if sessions == 0 {
            // Not "all sessions": a blank one is not on disk, and needs not be.
            self.notify("every session with history is already on disk");
        } else {
            self.notify(format!(
                // Sessions first: a lazy clone or merge is saved with no turn
                // of its own, and "0 turn(s) written" read as nothing saved
                // (Reviewer F, round 18).
                "{sessions} session(s) saved, {written} new turn(s)"
            ));
        }
    }
}
