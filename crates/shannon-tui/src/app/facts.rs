//! **What each fact is now** (`crate::facts`) — the numbers an alert's
//! `when` asks of the app, each read from the state the status row's values
//! already read, so a fact and the value beside it agree.
//!
//! Asked at draw, for an alert whose `when` names the fact, and before each
//! frame for an alert that rings (`App::settle_rings`, B2.3), which keeps
//! what each held until the next: the cost is those alerts × the sessions
//! open × a lookup by number (`Buffers::get`, logarithmic since review stop
//! 10, B 1 — it was linear, and the whole quadratic), never the history
//! (§5.2).

use ratatui::style::Style;

use super::{App, Stage, Work};
use crate::facts::{AppFact, Fact, SessionFact};
use crate::status_line::{Source, Value, Var};
use crate::style::Ink;
use crate::wm::BufferId;

/// **One session, as its alerts ask it** — its facts, the app's, and the
/// theme's roles; or the app alone (`session: None`). The one place a fact
/// is asked of a session or of the app (review stop 9, C 1: the row, this
/// and the rings each wrote the match). Its consumers: what an alert marks
/// on a session's own — its windows' border (`Alerts::border`), its `:ls`
/// line (`Alerts::mark`) —, the row's facts (`view::Row`, its aim's), and
/// the rings (`App::settle_rings`, each session's and the app's). It says
/// no values: none of those is text.
pub(crate) struct OfSession<'a> {
    app: &'a App,
    session: Option<BufferId>,
}

impl<'a> OfSession<'a> {
    pub(crate) fn new(app: &'a App, session: BufferId) -> Self {
        OfSession {
            app,
            session: Some(session),
        }
    }

    /// The app alone, for an alert on the app's fact — which no session
    /// holds for, so a session's fact asked of it holds nothing.
    pub(crate) fn app(app: &'a App) -> Self {
        OfSession { app, session: None }
    }
}

impl Source for OfSession<'_> {
    fn value(&mut self, _: Var) -> Value {
        Value::default()
    }

    fn fact(&mut self, fact: Fact) -> u64 {
        match (fact, self.session) {
            (Fact::Session(fact), Some(session)) => self.app.session_fact(fact, session),
            (Fact::Session(_), None) => 0,
            (Fact::App(fact), _) => self.app.app_fact(fact),
        }
    }

    fn ink(&mut self, ink: Ink) -> Style {
        self.app.theme().ui(ink)
    }
}

impl App {
    /// **What `fact` is in `buffer`'s session.**
    pub(crate) fn session_fact(&self, fact: SessionFact, buffer: BufferId) -> u64 {
        let n = match fact {
            // Every decision, as the session's gate counts them: a call whose
            // record is gone holds nothing (`decision_holds`).
            SessionFact::Decision => {
                let calls = (self.decisions.calls_of(buffer))
                    .filter(|id| self.tool_call(id).is_some())
                    .count();
                calls + usize::from(self.decisions.review(buffer).is_some())
            }
            SessionFact::Review => usize::from(self.decisions.review(buffer).is_some()),
            SessionFact::CtxPercent => {
                let budget = self.context_budget(buffer);
                let percent = (self.session_of(buffer))
                    .and_then(|session| session.billing.context_percent(budget));
                return percent.map_or(0, u64::from);
            }
            SessionFact::Queued => (self.pending.iter())
                .filter(|(to, _)| *to == buffer)
                .count(),
            SessionFact::Running => (self.running_tools.keys())
                .filter(|id| self.tool_buffer(id) == Some(buffer))
                .count(),
            SessionFact::Answering => {
                let streaming = |work: &Work| {
                    matches!(
                        work,
                        Work::Turn {
                            stage: Stage::Streaming
                        }
                    )
                };
                usize::from(
                    (self.generations.iter()).any(|g| g.buffer == buffer && streaming(&g.work)),
                )
            }
            SessionFact::Unshown => usize::from(self.unshown(buffer)),
        };
        n as u64
    }

    /// **What `fact` is across the app.**
    pub(crate) fn app_fact(&self, fact: AppFact) -> u64 {
        let n = match fact {
            AppFact::Sessions => self.sessions_at_work().len(),
            AppFact::Children => self.running_children() + self.held_children(),
            AppFact::Sends => self.running_sends() + self.held_sends(),
            AppFact::UnshownCount => self.every_unshown().len(),
        };
        n as u64
    }
}
