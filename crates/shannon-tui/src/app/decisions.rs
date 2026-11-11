//! **The decisions waiting on you** — each session's: the call on its
//! decision pane, the calls behind it, a summary's review after them all,
//! whether its pane is kept open for its next, and when its keys start
//! answering
//! (`docs/DesignDocs/decision-panes-and-alerts.md` §3).
//!
//! **Every read and write of that state is here.** [`Decisions`]' fields
//! are private to this module, so the rest of the app asks by session or by
//! pane, and has no way to reach "the one call" behind the question it
//! means: four app-wide fields read at ~90 sites, each resolving "the call"
//! without saying whose, were the shape the design review found crossing
//! two sessions' decisions (DR-A #4).
//!
//! **A decision is its session's** (A1.3; Steven, 10-22: "The decision pane
//! should also be associated with panes it is making decisions for"). Each
//! session shows its own call in its own pane, its others waiting behind
//! it; one session's answer neither answers nor arms another's. It was one
//! pane for every session, and the bookkeeping that kept them apart — a
//! pane held for one session closed when another's call came, "next
//! another session's", the queue handing the pane across — is gone with it.

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use crossterm::event::KeyEvent;

use super::review::{Review, ReviewAction};
use super::{App, Cycle};
use crate::decision_page::DecisionPage;
use crate::permission_view::{ToolAction, ToolPrompt};
use crate::wm::{BufferId, PaneId};
use crate::workspace::PaneKind;

/// **A call waiting behind the one shown** — its session (the call's
/// `ToolCall::buffer`, fixed when the call was tracked and never moved), its
/// id, and the command as it was resolved when the call was admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Waiting {
    pub(crate) buffer: BufferId,
    pub(crate) id: String,
    pub(crate) command: String,
}

/// One session's decisions.
#[derive(Default)]
struct Session {
    /// The call on its decision pane.
    shown: Option<ToolPrompt>,
    /// The calls behind it, in the order they will be shown: a pane shows
    /// one at a time (a decision deserves undivided attention), so the
    /// others wait here rather than overwrite it.
    waiting: VecDeque<Waiting>,
    /// **A summary waiting for its review** (§4) — on the pane when no call
    /// is: calls first (Steven, 10-22). At most one: it holds its session
    /// from the moment it arrives, so no second distillation can start
    /// there until it is applied or dropped.
    review: Option<Review>,
    /// **Its pane is kept open for its next call.** Answering from the pane
    /// used to close it and hand focus to the prompt, so a message making
    /// five calls cost five trips back with `{n}<C-w>w`. Since a decision no
    /// longer takes focus when it arrives, nothing else could bring you
    /// back. Steven, 10-19: "staying in the approval box is fine… But make
    /// sure that you only do this for the approval box of the same chat".
    ///
    /// Set when a call is answered from its pane (`resolve_open_call`), and
    /// when `n`/`N` walk to another (`cycle_call`). It holds only while
    /// another of this session's calls waits, or its review
    /// (`settle_decision_wait`, at the event's end); that call then fills
    /// the pane (`decide_tool_call`), or the review takes it
    /// (`settle_decisions`, which clears it). An allowed command runs before
    /// the next call is asked, and the pane says so while it does. Any close
    /// ends it.
    kept_open: bool,
    /// **Hidden by `:hide`** (or a review's pane closed by you,
    /// [`Decisions::hide`]): its decisions wait with no pane until `:decide`
    /// brings them back — vim's `:hide`, which keeps a buffer where `:q` and
    /// `:close` abandon it (Steven, 10-22: "`:q` removes from the buffer
    /// while hide doesn't"). Forgotten once the session has nothing left, at
    /// the event's end ([`Decisions::forget_idle`]), so its next call opens
    /// a pane as any does.
    hidden: bool,
    /// **When what its pane shows — a call or the review — starts taking
    /// answers**: `tui.approval_delay_ms` after something new comes onto its
    /// pane — a call shown, the pane opened or come back, a review after
    /// the calls — once, by what the pane records it shows
    /// (`settle_decisions`, `Shown`); after `:decide` stands you in it; after
    /// it comes into view by room (`clamp_text_panes`); and that long again
    /// after each answer key pressed before then (`arming`) or any key the
    /// pane refuses (`handle_tool_key`'s tail) — five writers (review stop 8,
    /// D 5: `decide_tool_call` armed too, and a review in a new pane was
    /// armed twice). `None`: live.
    answers_from: Option<Instant>,
}

impl Session {
    /// Nothing shown, nothing waiting, no review, nothing kept open: no entry
    /// needed — whether hidden or not, which is said of what it holds.
    fn is_idle(&self) -> bool {
        self.shown.is_none() && self.waiting.is_empty() && self.review.is_none() && !self.kept_open
    }

    /// **Which kind its pane shows — calls first, exactly** (Steven, 10-22):
    /// its call; with none shown and none waiting, its review. A call
    /// waiting with none shown — an allowed one running before the next is
    /// asked — keeps the pane for the calls: the review comes after them
    /// all. The one statement of the rule; what is drawn and what is
    /// answered both read it.
    fn front(&self) -> Option<Front> {
        match (&self.shown, &self.review) {
            (Some(_), _) => Some(Front::Call),
            (None, Some(_)) if self.waiting.is_empty() => Some(Front::Review),
            _ => None,
        }
    }
}

/// Which kind a pane shows ([`Session::front`]).
#[derive(Clone, Copy)]
enum Front {
    Call,
    Review,
}

/// **What a decision pane shows** — the call shown, or with none the review:
/// calls first.
#[derive(Clone, Copy)]
pub(crate) enum OnPane<'a> {
    Call(&'a ToolPrompt),
    Review(&'a Review),
}

impl<'a> OnPane<'a> {
    /// Its page: sized, folded and drawn as any decision's is.
    pub(crate) fn page(self) -> &'a dyn DecisionPage {
        match self {
            OnPane::Call(prompt) => prompt,
            OnPane::Review(review) => review,
        }
    }

    /// **What `key` answers**, of whichever kind — the one answer the pane's
    /// answer path and its no-room guard both read (design review DR-B #8):
    /// a review's `<CR>` is no character, so a guard asking of characters
    /// would let it through on a pane with no room.
    pub(crate) fn resolve(self, key: &KeyEvent) -> Option<Answer> {
        match self {
            OnPane::Call(prompt) => prompt.resolve(key).map(Answer::Call),
            OnPane::Review(review) => review.resolve(key).map(Answer::Review),
        }
    }
}

/// **What a decision pane shows, by identity** — a call by its id, or the
/// review — recorded on the pane when it came there (`DecisionView::shows`), so
/// "something new came onto the pane" is one comparison
/// (`settle_decisions`): a pane opening or coming back, the next call
/// filling one kept open, a review after its session's calls. It was four
/// writers, and a review in a new pane was fitted and armed twice (review
/// stop 8, D 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Shown {
    Call(String),
    Review,
}

/// **An answer to what a decision pane shows** ([`OnPane::resolve`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answer {
    Call(ToolAction),
    Review(ReviewAction),
}

/// **Every session's decisions** — read and written only through its
/// methods. A session with nothing has no entry.
#[derive(Default)]
pub(crate) struct Decisions {
    sessions: BTreeMap<BufferId, Session>,
}

impl Decisions {
    /// `buffer`'s entry, made if it has none.
    fn of(&mut self, buffer: BufferId) -> &mut Session {
        self.sessions.entry(buffer).or_default()
    }

    /// `buffer`'s entry gone, if it holds nothing — and is not hidden: that
    /// one is forgotten at the event's end ([`forget_idle`](Self::forget_idle)),
    /// as within an event its last waiting call is taken off to be shown
    /// (`present_next_call`), and for that moment it holds nothing. Forgotten
    /// here, its next call was shown with a pane.
    fn tidy(&mut self, buffer: BufferId) {
        if (self.sessions.get(&buffer)).is_some_and(|s| s.is_idle() && !s.hidden) {
            self.sessions.remove(&buffer);
        }
    }

    /// **Every session with nothing left forgotten**, a hidden one too — at
    /// the event's end (`settle_decisions`): a session whose decisions are
    /// all answered is hidden no more, and its next call opens a pane as any
    /// does.
    pub(crate) fn forget_idle(&mut self) {
        self.sessions.retain(|_, session| !session.is_idle());
    }

    // --- the call shown -------------------------------------------------------

    /// The call shown for `buffer`'s session, if one is.
    pub(crate) fn shown(&self, buffer: BufferId) -> Option<&ToolPrompt> {
        self.sessions.get(&buffer)?.shown.as_ref()
    }

    /// [`shown`](Self::shown), to change.
    pub(crate) fn shown_mut(&mut self, buffer: BufferId) -> Option<&mut ToolPrompt> {
        self.sessions.get_mut(&buffer)?.shown.as_mut()
    }

    /// **Every call shown**, with its session.
    pub(crate) fn every_shown(&self) -> impl Iterator<Item = (BufferId, &ToolPrompt)> {
        (self.sessions.iter()).filter_map(|(b, s)| s.shown.as_ref().map(|p| (*b, p)))
    }

    /// **Whether a call of `buffer`'s must wait** behind one shown — its
    /// own: another session's call never holds this session's pane. Not
    /// behind its review: calls come first (Steven, 10-22), and one arriving
    /// while a review is up is one you typed (the review holds its session,
    /// so no turn runs there), shown with its keys waiting as any call's.
    pub(crate) fn pane_taken(&self, buffer: BufferId) -> bool {
        self.shown(buffer).is_some()
    }

    // --- what the pane shows --------------------------------------------------

    /// **What `buffer`'s pane shows** ([`Session::front`]): its call, or its
    /// review — or would, for a session whose pane is hidden or in no
    /// window.
    pub(crate) fn on_pane(&self, buffer: BufferId) -> Option<OnPane<'_>> {
        let session = self.sessions.get(&buffer)?;
        match session.front()? {
            Front::Call => session.shown.as_ref().map(OnPane::Call),
            Front::Review => session.review.as_ref().map(OnPane::Review),
        }
    }

    /// [`on_pane`](Self::on_pane)'s page, to move through — by the same
    /// rule: it had its own copy of "calls first", which a mutant could
    /// break with the suite green, so the keys could answer a review the
    /// pane did not draw (review stop 8, A 2 / C 1).
    pub(crate) fn page_mut(&mut self, buffer: BufferId) -> Option<&mut dyn DecisionPage> {
        let session = self.sessions.get_mut(&buffer)?;
        match session.front()? {
            Front::Call => session.shown.as_mut().map(|p| p as &mut dyn DecisionPage),
            Front::Review => session.review.as_mut().map(|r| r as &mut dyn DecisionPage),
        }
    }

    /// [`on_pane`](Self::on_pane), by identity.
    pub(crate) fn shown_on_pane(&self, buffer: BufferId) -> Option<Shown> {
        Some(match self.on_pane(buffer)? {
            OnPane::Call(prompt) => Shown::Call(prompt.tool_use_id().to_string()),
            OnPane::Review(_) => Shown::Review,
        })
    }

    /// **Every session with something on its pane**, and what.
    pub(crate) fn every_on_pane(&self) -> impl Iterator<Item = (BufferId, OnPane<'_>)> {
        (self.sessions.keys()).filter_map(|&b| Some((b, self.on_pane(b)?)))
    }

    // --- what holds the session -----------------------------------------------

    /// **`buffer`'s calls, by id** — the one shown, then the ones waiting.
    pub(crate) fn calls_of(&self, buffer: BufferId) -> impl Iterator<Item = &str> {
        let session = self.sessions.get(&buffer);
        let shown = session
            .and_then(|s| s.shown.as_ref())
            .map(|p| p.tool_use_id());
        let waiting = session.into_iter().flat_map(|s| s.waiting.iter());
        shown.into_iter().chain(waiting.map(|w| w.id.as_str()))
    }

    /// **Whether a decision holds `buffer`'s session** — a call shown, a call
    /// waiting, or its review: what a gate reads (a prompt, a job, a turn's
    /// resume), where [`on_pane`](Self::on_pane) is what you owe NOW. The
    /// two differ while an allowed command runs and the session's next call
    /// waits unasked: held, nothing to answer yet (review stop 8, D 1).
    ///
    /// `live` says whether a call's record still stands: an entry whose
    /// record is gone holds nothing — it would hold its session forever.
    /// Read from the session's own entry; it scanned every session's calls
    /// for ones recorded as this session's, which the per-session storage
    /// (A1.3) made the same set.
    pub(crate) fn holds(&self, buffer: BufferId, live: impl Fn(&str) -> bool) -> bool {
        self.review(buffer).is_some() || self.calls_of(buffer).any(live)
    }

    // --- the review -----------------------------------------------------------

    /// `buffer`'s review waiting, if one is.
    pub(crate) fn review(&self, buffer: BufferId) -> Option<&Review> {
        self.sessions.get(&buffer)?.review.as_ref()
    }

    /// [`review`](Self::review), to change.
    pub(crate) fn review_mut(&mut self, buffer: BufferId) -> Option<&mut Review> {
        self.sessions.get_mut(&buffer)?.review.as_mut()
    }

    /// **Every review waiting**, with its session.
    pub(crate) fn every_review(&self) -> impl Iterator<Item = (BufferId, &Review)> {
        (self.sessions.iter()).filter_map(|(b, s)| s.review.as_ref().map(|r| (*b, r)))
    }

    /// Put `review` to `buffer`'s session, behind its calls.
    pub(crate) fn add_review(&mut self, buffer: BufferId, review: Review) {
        debug_assert!(self.review(buffer).is_none(), "a review replaced");
        self.of(buffer).review = Some(review);
    }

    /// Take `buffer`'s review — applied, dropped, or ended with its session.
    pub(crate) fn take_review(&mut self, buffer: BufferId) -> Option<Review> {
        let taken = self.sessions.get_mut(&buffer)?.review.take();
        self.tidy(buffer);
        taken
    }

    /// Show `prompt` for `buffer`. Only where its pane is free
    /// ([`pane_taken`](Self::pane_taken)): a decision on screen is never
    /// replaced under the cursor (Reviewer E).
    pub(crate) fn show(&mut self, buffer: BufferId, prompt: ToolPrompt) {
        debug_assert!(!self.pane_taken(buffer), "a shown call replaced");
        self.of(buffer).shown = Some(prompt);
    }

    /// Take `buffer`'s shown call off its pane.
    pub(crate) fn take_shown(&mut self, buffer: BufferId) -> Option<ToolPrompt> {
        let taken = self.sessions.get_mut(&buffer)?.shown.take();
        self.tidy(buffer);
        taken
    }

    // --- the calls waiting ----------------------------------------------------

    /// Put a call behind its session's shown one — at the back, or at the
    /// front.
    pub(crate) fn wait(&mut self, waiting: Waiting, at_front: bool) {
        let queue = &mut self.of(waiting.buffer).waiting;
        match at_front {
            true => queue.push_front(waiting),
            false => queue.push_back(waiting),
        }
    }

    /// **Every call waiting**, every session's, each session's in order.
    pub(crate) fn every_waiting(&self) -> impl Iterator<Item = &Waiting> {
        self.sessions.values().flat_map(|s| s.waiting.iter())
    }

    /// `buffer`'s calls waiting, in order.
    pub(crate) fn waiting_of(&self, buffer: BufferId) -> impl Iterator<Item = &Waiting> {
        self.sessions
            .get(&buffer)
            .into_iter()
            .flat_map(|s| s.waiting.iter())
    }

    /// **The next call for `buffer`'s pane**, from the front or the back.
    pub(crate) fn next_waiting(&mut self, buffer: BufferId, from: Cycle) -> Option<Waiting> {
        let queue = &mut self.sessions.get_mut(&buffer)?.waiting;
        let next = match from {
            Cycle::Next => queue.pop_front(),
            Cycle::Back => queue.pop_back(),
        };
        self.tidy(buffer);
        next
    }

    /// The call `id` waits no longer — its record forgotten.
    pub(crate) fn unqueue(&mut self, id: &str) {
        let held: Vec<BufferId> = self.sessions.keys().copied().collect();
        for buffer in held {
            self.of(buffer).waiting.retain(|w| w.id != id);
            self.tidy(buffer);
        }
    }

    /// **Take every call waiting that `which` picks**, every session's.
    pub(crate) fn withdraw(&mut self, which: impl Fn(&Waiting) -> bool) -> Vec<Waiting> {
        let mut taken = Vec::new();
        let held: Vec<BufferId> = self.sessions.keys().copied().collect();
        for buffer in held {
            let queue = std::mem::take(&mut self.of(buffer).waiting);
            let (out, kept): (VecDeque<Waiting>, VecDeque<Waiting>) =
                queue.into_iter().partition(&which);
            self.of(buffer).waiting = kept;
            taken.extend(out);
            self.tidy(buffer);
        }
        taken
    }

    // --- the pane kept open, and its arming -----------------------------------

    /// Whether `buffer`'s pane is kept open for its next call.
    pub(crate) fn kept_open(&self, buffer: BufferId) -> bool {
        self.sessions.get(&buffer).is_some_and(|s| s.kept_open)
    }

    /// Every session whose pane is kept open.
    pub(crate) fn every_kept_open(&self) -> Vec<BufferId> {
        (self.sessions.iter())
            .filter(|(_, s)| s.kept_open)
            .map(|(b, _)| *b)
            .collect()
    }

    /// Keep `buffer`'s pane open for its next call — or not.
    pub(crate) fn keep_open(&mut self, buffer: BufferId, open: bool) {
        self.of(buffer).kept_open = open;
        self.tidy(buffer);
    }

    /// **Whether `buffer` has anything for a pane**: what its pane shows
    /// ([`Session::front`]), or the pane kept open for its next call — and
    /// is not hidden. By `front`, not its own copy: it counted a review
    /// behind waiting calls, which no pane shows, and the review's arrival
    /// said "in the pane beside it" where none opened (review stop 8, D 1).
    pub(crate) fn wants_a_pane(&self, buffer: BufferId) -> bool {
        (self.sessions.get(&buffer))
            .is_some_and(|s| !s.hidden && (s.front().is_some() || s.kept_open))
    }

    /// Every session that has something for a pane.
    pub(crate) fn every_wanting_a_pane(&self) -> Vec<BufferId> {
        (self.sessions.keys().copied())
            .filter(|&b| self.wants_a_pane(b))
            .collect()
    }

    /// **Every session with decisions**, a pane or not — hidden ones
    /// included, which [`every_wanting_a_pane`](Self::every_wanting_a_pane)
    /// leaves out.
    pub(crate) fn every_session(&self) -> Vec<BufferId> {
        self.sessions.keys().copied().collect()
    }

    // --- hidden ---------------------------------------------------------------

    /// **Hide `buffer`'s decisions** — `:hide` on its pane, a call or a
    /// review shown (`hide_decision`), or a review's pane closed by you
    /// (`reconcile_pane_state`): no pane opens for them until
    /// [`reveal`](Self::reveal). (Not kept open too: what is shown has taken
    /// that wait.)
    pub(crate) fn hide(&mut self, buffer: BufferId) {
        self.of(buffer).hidden = true;
    }

    /// Bring `buffer`'s decisions back to a pane — `:decide`.
    pub(crate) fn reveal(&mut self, buffer: BufferId) {
        if let Some(session) = self.sessions.get_mut(&buffer) {
            session.hidden = false;
        }
    }

    /// When the answer keys of what `buffer`'s pane shows — a call or a
    /// review — go live, if they are not.
    pub(crate) fn answers_from(&self, buffer: BufferId) -> Option<Instant> {
        self.sessions.get(&buffer)?.answers_from
    }

    /// Hold `buffer`'s answer keys until `until`.
    pub(crate) fn arm(&mut self, buffer: BufferId, until: Instant) {
        self.of(buffer).answers_from = Some(until);
    }
}

impl App {
    /// **Whether a decision is in a pane** — a call or a review, drawn where
    /// you can answer it: what an open screen that takes every key must not
    /// cover. Setup's refusal and its bar ask it, one question: the bar
    /// asked of any decision, a hidden one too, so over a `:hide`n call it
    /// said one waited behind the screen, where nothing was, its own keys
    /// gone (review stop 8, C 5). One hidden, or in no window, is under
    /// nothing setup covers; the row points to it (`:decide N`). **In the
    /// tab you are in**: setup covers this tab's panes, and one parked in
    /// another tab is under nothing it covers either (T2;
    /// `decision_pane_of` finds a pane in any tab).
    pub(crate) fn decision_in_a_pane(&self) -> bool {
        (self.decisions.every_on_pane()).any(|(b, _)| {
            (self.decision_pane_of(b)).is_some_and(|p| self.workspace.window(p).is_some())
        })
    }

    /// **The tab `buffer`'s decision pane is parked in**, numbered as `gt`
    /// counts them (from 1) — `None` for one in the tab you are in, or with
    /// no pane. What the row says of it instead of its keys (`$hidden`'s
    /// "DECIDE in tab N", `tabs.md` Q1): its keys answer only where it is.
    pub(crate) fn decision_tab(&self, buffer: BufferId) -> Option<usize> {
        let tab = self.tab_of(self.decision_pane_of(buffer)?)?;
        (tab != self.tabs.current()).then_some(tab + 1)
    }

    /// **Every decision on its pane** — a call shown, or a review — with its
    /// session: what the row says (`$decide`, `$hidden`).
    pub(crate) fn every_decision_up(&self) -> impl Iterator<Item = (BufferId, OnPane<'_>)> {
        self.decisions.every_on_pane()
    }

    /// **The session a decision pane decides for** — its own record, set
    /// when it opened (`DecisionView::decides`), while that session still has
    /// something for it ([`Decisions::wants_a_pane`]): a call shown, a review,
    /// or the pane kept open for its next, and not hidden. `None` for any other
    /// pane, and for one whose session has nothing — which, between events,
    /// is none: `App::settle_decisions` closes such a pane at every event's
    /// end, so the gate holds there and no test can see it (review stop 10,
    /// E 3: mutated to `Some`, every test passed).
    pub(crate) fn decides_for(&self, pane: PaneId) -> Option<BufferId> {
        if self.kind_of(pane) != Some(PaneKind::Tool) {
            return None;
        }
        let (buffer, _) = self.window_of(pane)?.decides()?;
        self.decisions.wants_a_pane(buffer).then_some(buffer)
    }

    /// **The manual's section for `pane`** — its kind's (`PaneKind::
    /// help_topic`), except a decision pane showing a review: its keys are
    /// not a call's, and the call's chapter teaches `d` as the harmless deny
    /// where on a review it drops paid work (review stop 8, F 1).
    pub(crate) fn help_topic(&self, pane: PaneId) -> &'static str {
        match self.on_pane_in(pane) {
            Some(OnPane::Review(_)) => "review",
            _ => (self.kind_of(pane)).map_or("", PaneKind::help_topic),
        }
    }

    /// **What decision pane `pane` shows** — a call, or a review.
    pub(crate) fn on_pane_in(&self, pane: PaneId) -> Option<OnPane<'_>> {
        self.decisions.on_pane(self.decides_for(pane)?)
    }

    /// **The name `buffer`'s decision is said under** on the status row —
    /// `None` while it is the session the prompt goes to, which needs none
    /// (`name_off_the_aim`, the one naming rule).
    pub(crate) fn decision_name(&self, buffer: BufferId) -> Option<String> {
        self.name_off_the_aim(buffer)
    }

    /// **The session of the decision pane you stand in**, if you stand in one.
    pub(crate) fn focused_decision(&self) -> Option<BufferId> {
        self.decides_for(self.workspace.focused())
    }

    /// **The decision pane of `buffer`'s session**, if it has one — the pane
    /// showing its call or its review, or kept open for its next — found by the pane's own
    /// record ([`decides_for`](Self::decides_for)), never by its kind. **In
    /// whichever tab it is** (T2): one decision has one pane, and a second
    /// opened here for one parked elsewhere would answer it twice.
    pub(crate) fn decision_pane_of(&self, buffer: BufferId) -> Option<PaneId> {
        (self.windows())
            .map(|(pane, _)| pane)
            .find(|&pane| self.decides_for(pane) == Some(buffer))
    }

    /// **When a decision pane's keys come alive**, the soonest, while one is
    /// still waiting — the event loop's wake, to draw its legend live again
    /// (review stop 6, F 4).
    pub(crate) fn arming_ends_at(&self) -> Option<Instant> {
        (self.decisions.every_on_pane())
            .filter_map(|(b, _)| Some((b, self.decision_pane_of(b)?)))
            .filter(|&(_, pane)| self.answer_keys(pane) == super::AnswerKeys::Waiting)
            .filter_map(|(b, _)| self.decisions.answers_from(b))
            .min()
    }
}
