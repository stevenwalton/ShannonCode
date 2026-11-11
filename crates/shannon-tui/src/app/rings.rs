//! **What the alerts send out of the terminal, frame by frame** (B2.3) —
//! the edges of each alert with a bell or a notification, gathered before a
//! frame is drawn and written after it (`crate::ring` holds the words, the
//! door and the bytes).

use std::collections::HashMap;

use super::news::Edges;
use super::{App, OfSession, SessionBuffer};
use crate::facts::Fact;
use crate::status_line::RuleKey;
use crate::wm::BufferId;

impl App {
    /// **What each ringing alert sends this frame** — for each one that
    /// BEGAN to hold since the last frame, for each session it holds for (or
    /// for the app's fact): the bell, and its notification's words through
    /// the door (`ring::Outbox::send`), into the outbox the event loop writes after
    /// the draw (§5.3, DR-B #15). Once per (alert, session) rising edge: one
    /// still holding rings nothing more.
    ///
    /// **Silent where it has no last frame to compare with** ([`Edges`]):
    /// the first frame, an alert new or rewritten (a new [`RuleKey`]), a
    /// session given to you (`App::hold`) — what a session holds when it
    /// opens is not news
    /// (Steven, 10-23: ringing on `:open` "would be a bit noisy and
    /// encourage people to turn off the alerts entirely"). A change you made
    /// was taken as it left things (`App::yours`). A session that ARRIVED
    /// held nothing before it, so what holds of it rises.
    ///
    /// After `settle_seen`, which `unshown` reads — by type: only it makes
    /// the `Sighted` this takes. Asked only of the alerts that send
    /// something (`Alerts::ringing`): the cost is those alerts × the
    /// sessions open × a lookup by number (`Buffers::get`, a binary search),
    /// never the history. The lookup was a linear scan, so this said
    /// "alerts × sessions" of a cost that was quadratic in the buffers a run
    /// had opened (review stop 10, B 1, measured).
    pub(super) fn settle_rings(&mut self, _: super::news::Sighted) {
        // This frame's edges, the app's and each session's, and what they
        // send — kept apart until gathered, then handed over whole: the
        // facts and the scrub borrow the app while they are.
        let app_now = self.app_edges();
        let sessions_now: Vec<HashMap<RuleKey, bool>> = (self.buffers.sessions())
            .map(|b| self.session_edges(b.id))
            .collect();
        let mut sends = crate::ring::Outbox::default();
        let masters = &self.config.tui.alerts;
        for ringing in self.status_alerts().ringing() {
            // Each subject: what it holds now, what it held at the last
            // frame, and which session it is.
            let subjects: Vec<(bool, Option<bool>, Option<&SessionBuffer>)> = match ringing.fact() {
                Fact::Session(_) => (self.buffers.sessions().zip(&sessions_now))
                    .map(|(b, now)| {
                        let holds = now.get(&ringing.key).copied().unwrap_or_default();
                        (holds, b.told.edges.was(&ringing.key), Some(b))
                    })
                    .collect(),
                Fact::App(_) => {
                    let holds = app_now.get(&ringing.key).copied().unwrap_or_default();
                    vec![(holds, self.rang.was(&ringing.key), None)]
                }
            };
            for (holds, was, session) in subjects {
                if !holds || was != Some(false) {
                    continue;
                }
                if ringing.bell && masters.bell {
                    sends.ring();
                }
                if let Some(words) = ringing.notify.filter(|_| masters.notify) {
                    let session = session.map(|b| self.session_label(b.conversation.episode().id));
                    let said = words.say(ringing.name(), session.as_deref());
                    sends.send(&said, |text| self.scrub(text));
                }
            }
        }
        for (buffer, now) in self.buffers.sessions_mut().zip(sessions_now) {
            buffer.told.edges = Edges::Held(now);
        }
        self.rang = Edges::Held(app_now);
        self.outbox.take_from(sends);
    }

    /// **What each alert with a bell or a notification on a session's fact
    /// holds for `buffer` now**, by its identity ([`RuleKey`]) — what a
    /// frame compares with the last (`settle_rings`), and what a change you
    /// made is compared across (`App::yours`), so what it moves rings
    /// nothing.
    pub(super) fn session_edges(&self, buffer: BufferId) -> HashMap<RuleKey, bool> {
        self.edges_of(
            |fact| matches!(fact, Fact::Session(_)),
            &mut OfSession::new(self, buffer),
        )
    }

    /// **What each alert with a bell or a notification on an app fact holds
    /// now** — [`session_edges`](Self::session_edges)' twin, for the app's
    /// edges (`App::rang`): `unshown_count`, `sessions`, `sends`,
    /// `children`. Compared across a change you made too: left out, a stop
    /// rang an `unshown_count` alert (review stop 9, G 1).
    pub(super) fn app_edges(&self) -> HashMap<RuleKey, bool> {
        self.edges_of(
            |fact| matches!(fact, Fact::App(_)),
            &mut OfSession::app(self),
        )
    }

    /// What each ringing alert whose fact `scope` takes holds, as `source`
    /// answers.
    fn edges_of(
        &self,
        scope: impl Fn(Fact) -> bool,
        source: &mut OfSession<'_>,
    ) -> HashMap<RuleKey, bool> {
        (self.status_alerts().ringing())
            .filter(|ringing| scope(ringing.fact()))
            .map(|ringing| {
                let holds = ringing.holds(source);
                (ringing.key, holds)
            })
            .collect()
    }

    /// **What the alerts send, as the terminal's bytes** — the outbox
    /// emptied, for the event loop to write after the frame is drawn (a bell
    /// and an `OSC 9` move no cursor, so the frame stands). Empty when
    /// nothing rang.
    pub(crate) fn take_rings(&mut self) -> Vec<u8> {
        let outbox = std::mem::take(&mut self.outbox);
        crate::ring::encode(&outbox, self.config.tui.alerts.notify_via)
    }
}
