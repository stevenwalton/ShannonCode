//! **What is news, and what you have been told of each session** (§5.3–5.4)
//! — the one door that gives a buffer a session ([`App::hold`]), the one for a
//! change you made to a transcript ([`App::yours`]), and what each frame sees
//! ([`App::settle_seen`], [`App::unshown`]). Whether a change is news is said
//! by whoever makes it, as a [`Landing`]: what you asked for is not, and what
//! arrives is, wherever it lands (Steven, 10-23: "your own actions are not
//! news. You explicitly did it").
//!
//! It was inferred after the fact, from "changed since a pane drew it, and
//! idle", so every door that changed a transcript decided by accident: a
//! `:delete` or a turn delete of a session no pane draws rang "news in" the
//! fresh session (review stop 9, A F1), a `<C-c>` on a hidden `:Send` rang
//! for your own echo (D1), and the same arrived summary was news `Over` a
//! hidden buffer and silent `Apart` (A F6). The rings read the edges kept
//! here (`rings.rs`).

use std::collections::HashMap;

use super::*;
use crate::status_line::RuleKey;

/// **What you have been told of one session**: what a pane last drew of its
/// transcript, and what each ringing alert held for it at the last frame.
/// Set whole when a buffer is given a session ([`App::hold`]), moved by a
/// change you made ([`App::yours`]) and by each frame (`settle_seen`,
/// `settle_rings`) — never field by field, so the two halves cannot
/// disagree: a session seen anew whose edges were kept rang nothing for
/// what it held (review stop 9, D2's probe).
pub(super) struct Told {
    /// The revision a pane last drew ([`Output::revision`]). `None` until
    /// the next frame, which takes it as it is without a word — what a
    /// session held when you opened it is not news (Steven, 10-23: ringing
    /// on `:open` "would be a bit noisy and encourage people to turn off the
    /// alerts entirely"). One that arrived has been seen EMPTY
    /// ([`Output::EMPTY`]), so anything in it is news until a pane draws it.
    seen: Option<u64>,
    pub(super) edges: Edges,
}

/// **What each alert with a bell or a notification held for one subject at
/// the last frame**, by its identity ([`RuleKey`]) — a session's (on
/// [`Told`]) or the app's (`App::rang`).
pub(super) enum Edges {
    /// No last frame to compare with: what holds at the next is taken without
    /// a word — the first frame, a session given to you.
    Unseeded,
    /// It arrived: nothing held before it, so whatever holds at the next
    /// frame has risen.
    Nothing,
    /// At the last frame, or just after a change you made
    /// ([`App::yours`]). An alert missing from it — new, rewritten, or not
    /// ringing then — is taken without a word too.
    Held(HashMap<RuleKey, bool>),
}

impl Edges {
    /// What the alert `key` names held at the last frame — `None` where
    /// there is nothing to compare with, so nothing rises.
    pub(super) fn was(&self, key: &RuleKey) -> Option<bool> {
        match self {
            Edges::Unseeded => None,
            Edges::Nothing => Some(false),
            Edges::Held(held) => held.get(key).copied(),
        }
    }

    /// **What a change you made moved, taken as it left things** — each
    /// alert whose holding went from `was` to something else in `now` is
    /// held at what it holds now, so no frame sees it rise; every other edge
    /// is kept as the last frame left it, so what arrived since still rises.
    /// Taken whole, a re-take swallowed an arrival elsewhere in the same
    /// frame (review stop 9, G 1).
    fn absorb(&mut self, was: &HashMap<RuleKey, bool>, now: &HashMap<RuleKey, bool>) {
        let moved = now
            .iter()
            .filter(|&(key, holds)| was.get(key) != Some(holds));
        match self {
            // The next frame takes everything without a word anyway.
            Edges::Unseeded => {}
            // Nothing held before it but what this moved.
            Edges::Nothing => {
                let mut held: HashMap<RuleKey, bool> =
                    now.keys().map(|key| (key.clone(), false)).collect();
                held.extend(moved.map(|(key, &holds)| (key.clone(), holds)));
                *self = Edges::Held(held);
            }
            Edges::Held(held) => held.extend(moved.map(|(key, &holds)| (key.clone(), holds))),
        }
    }
}

impl Told {
    /// What you have been told of a session a buffer is given, `landing`
    /// saying whether you asked for it.
    pub(super) fn of(landing: Landing) -> Told {
        match landing {
            Landing::Asked => Told {
                seen: None,
                edges: Edges::Unseeded,
            },
            Landing::Arrived => Told {
                seen: Some(Output::EMPTY),
                edges: Edges::Nothing,
            },
        }
    }

    /// Has the transcript, at `revision`, news a pane has not drawn?
    fn unseen(&self, revision: u64) -> bool {
        self.seen.is_some_and(|seen| seen != revision)
    }

    /// A frame drew the transcript at `revision` (`drawn`), or did not: one
    /// not seen yet is taken as it is either way.
    fn sighted(&mut self, revision: u64, drawn: bool) {
        if drawn || self.seen.is_none() {
            self.seen = Some(revision);
        }
    }

    /// **A change you made, from `before` to `after`**: what you had seen
    /// before it you have seen after it, so news it already held stays news
    /// — a reply you stopped half-read.
    fn follow(&mut self, before: u64, after: u64) {
        if self.seen == Some(before) {
            self.seen = Some(after);
        }
    }
}

/// **This frame's sight is settled** — what [`App::settle_seen`] returns
/// and `settle_rings` takes, so the rings cannot be asked first: asked first,
/// a reply you watched end was unshown for that one frame, and the shipped
/// `unshown_session` rang for it (review stop 9, C 2). Made here alone.
#[must_use = "the rings read what the frame showed: hand this to `settle_rings`"]
pub(super) struct Sighted(());

impl App {
    /// **The one door for "this buffer holds a session now"** — a new buffer
    /// (`into: None`, minted here) or one whose session is replaced. Built
    /// whole ([`SessionBuffer::new`]), so nothing of the session before
    /// survives in it: what you were told of it, its last write's failure (a
    /// clone landed over a session whose write had failed said at `:qa` that
    /// ITS write failed — review stop 9, D2), what `:ls` counted. `landing`
    /// says whether you asked for it ([`Told::of`]).
    ///
    /// Its consumers: launch's buffer (`App::new`, the constructor called
    /// directly), `:new` (`blank_buffer`), a child (`spawn_child_job`), a
    /// session opened, forked or checked out, or landed apart or beside
    /// (`buffer_holding`), a session replaced in place (`adopt_session`: a
    /// landing over a buffer, a turn delete), and `:delete`
    /// (`reset_buffer`).
    pub(super) fn hold(
        &mut self,
        into: Option<BufferId>,
        contents: Contents,
        landing: Landing,
    ) -> BufferId {
        let Some(id) = into else {
            let session = |id| {
                super::buffers::Buffer::Session(Box::new(SessionBuffer::new(id, contents, landing)))
            };
            return self.buffers.add(session);
        };
        if let Some(buffer) = self.session_of_mut(id) {
            let kept = std::mem::replace(&mut buffer.told.edges, Edges::Unseeded);
            *buffer = SessionBuffer::new(id, contents, landing);
            // **Arrived over a buffer, its alerts' edges are the buffer's**:
            // `queued`, `decision` and `ctx_percent` are asked of the buffer,
            // so one holding throughout rang again for a continuation or an
            // unreviewed summary landing (review stop 9, G 5). Nothing held
            // before is for a buffer with no last frame to compare with.
            if landing == Landing::Arrived {
                buffer.told.edges = match kept {
                    Edges::Held(held) => Edges::Held(held),
                    Edges::Unseeded | Edges::Nothing => Edges::Nothing,
                };
            }
        }
        // The new session invalidates the cursor of every pane SHOWING it —
        // all of them were looking at a transcript that no longer exists —
        // and only those: a pane parked on another session is untouched by
        // this one being replaced.
        self.forget_cursors_on(id);
        self.chat_dirty = true;
        id
    }

    /// **A change you made to `buffer`'s transcript** — your echo, your stop
    /// — made by `change`, and not news (Steven, 10-23, of `<C-c>` on a
    /// hidden `:Send`: ring nothing, and mark it only if part of a reply had
    /// arrived). What you had seen before it you have seen after it
    /// ([`Told::follow`]), and what it moved — of the session's alerts and
    /// the app's (an `unshown_count` one) — is taken as it left things
    /// ([`Edges::absorb`]), so it rings nothing, while what arrives after
    /// it, or arrived since the last frame, still rises: taken at the next
    /// frame instead, a reply ending inside the frame that echoed your
    /// prompt rang nothing; and the app's alerts were left out, so a stop
    /// rang an `unshown_count` one (review stop 9, G 1). Its consumers: the
    /// echo of a turn and of a `:Send` (`start_turn_on`, `start_job_on`),
    /// and what an ending you asked for changes ([`ending`](Self::ending)).
    pub(super) fn yours<T>(&mut self, buffer: BufferId, change: impl FnOnce(&mut Self) -> T) -> T {
        let was = (self.session_edges(buffer), self.app_edges());
        self.yours_from(buffer, was, change)
    }

    /// [`yours`](Self::yours), from what held before the change (`was`: the
    /// session's edges, the app's) — taken by the caller where "before" is
    /// not now ([`ending`](Self::ending)).
    fn yours_from<T>(
        &mut self,
        buffer: BufferId,
        was: (HashMap<RuleKey, bool>, HashMap<RuleKey, bool>),
        change: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let Some(before) = self.session_of(buffer).map(|b| b.output.revision()) else {
            return change(self);
        };
        let (session_was, app_was) = was;
        let changed = change(self);
        if let Some(b) = self.session_of_mut(buffer) {
            let after = b.output.revision();
            b.told.follow(before, after);
        }
        // After `follow`: `unshown` reads what you have seen.
        let (session_now, app_now) = (self.session_edges(buffer), self.app_edges());
        if let Some(b) = self.session_of_mut(buffer) {
            b.told.edges.absorb(&session_was, &session_now);
        }
        self.rang.absorb(&app_was, &app_now);
        changed
    }

    /// **What work's ending does to `buffer`'s transcript, done by `end`** —
    /// yours when you ended it, news when it arrived ([`Outcome::landing`]).
    /// Its consumers: `resolve`'s turn and job arms, each ending inside it:
    /// its tool round torn down (a command still running is said `⚙
    /// cancelled`, `cancel_running_tools`) and its seal. The teardown ran
    /// outside it, so a `<C-c>` while a command ran marked your own stop as
    /// news (review stop 9, G 4). What streamed before the ending
    /// (`flush_stream_tail`, first) arrived; so did a failure's `[error: …]`,
    /// a `Failed` ending being one that arrived.
    pub(super) fn ending<T>(
        &mut self,
        buffer: BufferId,
        why: &Outcome,
        end: impl FnOnce(&mut Self) -> T,
    ) -> T {
        match why.landing() {
            // **From the moment before you ended it**: its generation is
            // already taken when the ending runs (`stop_work`'s
            // `take_generations`), so asked now, a reply half-read already
            // held as news, nothing moved, and the stop rang it.
            Landing::Asked => {
                self.ending_in = Some(buffer);
                let was = (self.session_edges(buffer), self.app_edges());
                self.ending_in = None;
                self.yours_from(buffer, was, end)
            }
            Landing::Arrived => end(self),
        }
    }

    /// **What each transcript the frame draws has now shown** — its
    /// revision, for every buffer a pane will draw with room to show
    /// something, so `unshown` (news since a pane last drew it,
    /// [`unshown`](Self::unshown)) is asked of the frame and not of the
    /// window tree: a pane the layout squeezes to nothing shows nothing
    /// (DR-B #14), a viewer lent over a transcript shows the viewer, and a
    /// frame that draws no panes, or draws them under a float
    /// ([`frame_draws`](Self::frame_draws)), shows none — nor does any
    /// frame while you are not looking ([`looked`](Self::looked)). A buffer
    /// not yet
    /// seen takes what it holds without a word ([`Told`]) — under a whole
    /// screen too, which is where launch begins.
    pub(super) fn settle_seen(&mut self, frame_w: u16, frame_h: u16) -> Sighted {
        let layout = self
            .workspace
            .frame_layout(Rect::new(0, 0, frame_w, frame_h));
        // **Drawn is not seen while you are not looking** (Steven, 10-24:
        // "the window not being in focus is not considered seen") — a reply
        // ending in the pane you sent it from, while you work in another
        // window for the hour the turn takes, is the news the bell is for.
        let panes =
            self.looking && matches!(self.frame_draws(), FrameDraws::Panes | FrameDraws::Panel(_));
        let drawn: Vec<BufferId> = (layout.panes.iter())
            .filter(|_| panes)
            .filter(|p| {
                let (w, h) = p.content_size();
                w > 0 && h > 0
            })
            .filter(|p| self.kind_of(p.id) == Some(PaneKind::Output))
            .filter_map(|p| self.session_for_pane(p.id).map(|b| b.id))
            .collect();
        for buffer in self.buffers.sessions_mut() {
            let revision = buffer.output.revision();
            (buffer.told).sighted(revision, drawn.contains(&buffer.id));
        }
        Sighted(())
    }

    /// **Whether you are looking, as `event` says** — the terminal's own
    /// report that it lost or gained focus (`?1004`, asked for in
    /// `terminal.rs`; under tmux, `focus-events on`), and any key, click or
    /// paste, which only someone looking makes: so a missed report heals on
    /// the next key, and an `$EDITOR` closed in this terminal leaves you
    /// looking. A resize says nothing.
    pub(super) fn looked(&mut self, event: &crossterm::event::Event) {
        use crossterm::event::Event;
        match event {
            Event::FocusLost => self.looking = false,
            Event::FocusGained | Event::Key(_) | Event::Mouse(_) | Event::Paste(_) => {
                self.looking = true;
            }
            Event::Resize(..) => {}
        }
    }

    /// **Is there news in `buffer` you have not seen?** — its transcript
    /// changed since a pane last drew it, by something that arrived, and
    /// nothing runs in it now: a `:Send` reply, a turn ending, a summary
    /// landing, in a session no window shows (F 4; §5.4): the `unshown`
    /// fact. Not before the first frame has seen it, nor for what you did
    /// to it ([`yours`](Self::yours)). Your own prompt's reply is news too,
    /// under a viewer (Steven, 10-23: "that's a useful signal").
    ///
    /// **Idle, and not merely changed** (Steven, 10-23, shipping its bell):
    /// held until the work ends, it rises when the reply is there to read;
    /// while it runs, the row says so (`$sessions` of a turn, `$sends` of a
    /// `:Send`).
    pub(crate) fn unshown(&self, buffer: BufferId) -> bool {
        !self.working(buffer)
            && (self.session_of(buffer)).is_some_and(|b| b.told.unseen(b.output.revision()))
    }

    /// Every session [`unshown`](Self::unshown), in buffer order.
    pub(crate) fn every_unshown(&self) -> Vec<BufferId> {
        (self.buffers.sessions())
            .map(|b| b.id)
            .filter(|&b| self.unshown(b))
            .collect()
    }
}
