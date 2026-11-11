//! **A summary waiting for your review** — a reviewed distillation's draft,
//! put to you as its session's decision
//! (`docs/DesignDocs/decision-panes-and-alerts.md` §4).
//!
//! It used to take the terminal: a reviewed `S` or `:compact` opened
//! `$EDITOR` the moment the model finished, over whatever you were typing.
//! Steven, D 4: "I think what might work best is using a subpane prompting
//! for review. Similar to how we use the tool ask pane … When the user
//! navigates to it they may accept the review and be brought to $EDITOR.
//! This is then an informed and expected event." So it waits in a decision
//! pane beside its session, as a call does, and `$EDITOR` is a key pressed
//! there.

use crossterm::event::{KeyCode, KeyEvent};

use super::{App, SquashTarget, SummaryWork};
use crate::decision_page::{DecisionPage, KeyRow, KeyTable};
use crate::paint::PageLine;
use crate::text_view::TextView;
use crate::wm::BufferId;

/// **A draft waiting for its review**: what it was made from and composes
/// into, and the page its pane shows.
pub(crate) struct Review {
    /// The distillation, done: its target, its sources, its seal, the
    /// instruction sent and the model's reply.
    work: Box<SummaryWork>,
    /// What it was made from, said: `2 turns of b3 5da64e5a`.
    made_from: String,
    /// What `$EDITOR` last did with it.
    edited: Edited,
    /// The draft, as the model wrote it, scrubbed, its markdown styled.
    text: TextView,
    /// The keys that answer it.
    legend: Option<PageLine>,
}

/// **A review's pane does not close over its draft** — said to `q` in it,
/// and to `:q`, `:close`, `<C-w>c` and `<C-w>q` on it (§3.6, vim's E37 for a
/// modified buffer): one sentence, which it was not — two wordings of one
/// refusal (review stop 8, C 8).
/// It led with "the summary draft is not applied", which read as what `q`
/// had done (review stop 8, F 13).
pub(super) const NOT_CLOSED: &str =
    "not closed — the summary draft waits: `:hide` keeps it for later, `d` (or `:q!`) drops it";

/// **How a waiting review is reached** (`App::review_reach`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reach {
    /// In its pane, beside its session — on screen, or in tab N, where a
    /// decision opens for a session shown only there (`tabs.md` Q1 (a)).
    Beside(Option<usize>),
    /// With no pane — hidden, or its session in no window: `:decide N`.
    Decide,
    /// Behind calls of its session still to come: after them.
    AfterCalls,
}

/// **What `$EDITOR` last did with a review** — each kept, and said on its
/// title, since paid work goes only by `d` (design review DR-B #9).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Edited {
    /// Not opened yet.
    Not,
    /// **Closed without a save** — which cannot be told from `:x` on a draft
    /// left as it was — or saved empty, the editor's cancel (A2.3; it was the
    /// `ApplySummary` question).
    Unsaved,
    /// **Saved, and the compose refused it** — the edit no longer parses, or
    /// the session changed: kept, so `<CR>` opens what you wrote again rather
    /// than the model's draft (review stop 8, A 1 / B 1: it was dropped,
    /// the draft with it).
    Refused(String),
}

/// **What a key in a review's pane does** (`Review::resolve`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewAction {
    /// `<CR>` or `e`: review it in `$EDITOR`; `:wq` applies what is saved.
    Edit,
    /// `a`: apply it as drafted, unreviewed.
    Apply,
    /// `d`: drop it, nothing composed.
    Drop,
}

impl Review {
    /// A review of `work`, made from what `made_from` says, its draft shown
    /// as `shown` — the reply, scrubbed by the caller (`App::scrub_shown`),
    /// as every line of the model's text shown is.
    pub(super) fn new(work: Box<SummaryWork>, made_from: String, shown: &str) -> Review {
        // The transcript's rule for the model's text on a TUI
        // (`output::sanitize_lines`): a tab a space, a control byte gone. It
        // was `lines()` alone, so a tab vanished and only ratatui stood
        // between an escape in a draft and the terminal (review stop 8, B 4).
        let lines = crate::output::sanitize_lines(shown);
        let styles = crate::markdown::style_lines(&lines);
        let page: Vec<PageLine> = (lines.iter().zip(&styles))
            .map(|(line, styles)| PageLine::of_cells(line, styles))
            .collect();
        let mut review = Review {
            work,
            made_from,
            edited: Edited::Not,
            text: TextView::new(page),
            legend: None,
        };
        review.relegend();
        review
    }

    /// The distillation it reviews.
    pub(super) fn work(&self) -> &SummaryWork {
        &self.work
    }

    /// **Test seam: the distillation, to change** — a target made stale or
    /// gone by a door the review's hold would refuse.
    #[cfg(test)]
    pub(super) fn work_mut_for_test(&mut self) -> &mut SummaryWork {
        &mut self.work
    }

    /// What it was made from: `2 turns of b3 5da64e5a`.
    pub(crate) fn made_from(&self) -> &str {
        &self.made_from
    }

    /// Closed in `$EDITOR` without a save: said, and kept — and an edit of
    /// yours the compose refused is kept with it: opened again and closed
    /// unsaved, it was replaced by "closed unsaved", and the next `<CR>`
    /// opened the model's draft, your edit gone (review stop 8, F 2's test).
    pub(crate) fn close_unsaved(&mut self) {
        if !matches!(self.edited, Edited::Refused(_)) {
            self.edited = Edited::Unsaved;
        }
        self.relegend();
    }

    /// **`saved` would not compose** — kept, to open again; saved empty, the
    /// editor's cancel, it is as closed unsaved (nothing to open again).
    fn refuse_edit(&mut self, saved: String) {
        self.edited = match saved.trim().is_empty() {
            true => Edited::Unsaved,
            false => Edited::Refused(saved),
        };
        self.relegend();
    }

    /// **What `$EDITOR` opens on** — your refused edit, else the instruction
    /// and the draft (`App::summary_seed`).
    fn seed(&self) -> String {
        match &self.edited {
            Edited::Refused(saved) => saved.clone(),
            Edited::Not | Edited::Unsaved => App::summary_seed(&self.work),
        }
    }

    /// Whether `$EDITOR` last saved an edit the compose refused — so `a`,
    /// which applies the model's draft, says your edit was not used.
    pub(super) fn edit_refused(&self) -> bool {
        matches!(self.edited, Edited::Refused(_))
    }

    /// **Its keys, from what the review is now** — the one table its legend,
    /// its short form, its refusal and its [`resolve`](Self::resolve) all
    /// read (review stop 8, C 8; F 13). The answers first, then what answers
    /// nothing, so a narrow pane cuts navigation, not `d drop` (F 3); and in
    /// the refused state `a` says whose draft it applies: "as drafted" read
    /// as the draft you had just written (F 2).
    ///
    /// `<CR>` is the reflexive key, and it is the informed one: it opens the
    /// draft, it applies nothing. Not `q`: a draft is paid work, and the key
    /// you press for a thing to go away must not throw it away (the pane
    /// says what does).
    pub(crate) fn key_table(&self) -> KeyTable<ReviewAction> {
        let (edit, apply) = match self.edited {
            Edited::Not => ("<CR> review", "a apply"),
            Edited::Unsaved => ("<CR> review again", "a apply"),
            Edited::Refused(_) => ("<CR> your edit", "a apply the model's draft"),
        };
        let open = &[KeyCode::Enter, KeyCode::Char('e')];
        let scroll = &[KeyCode::Char('j'), KeyCode::Char('k')];
        KeyTable::new(vec![
            KeyRow::new(open, Some(ReviewAction::Edit), edit, Some("<CR> review")),
            KeyRow::new(
                &[KeyCode::Char('a')],
                Some(ReviewAction::Apply),
                apply,
                Some("a apply"),
            ),
            KeyRow::new(
                &[KeyCode::Char('d')],
                Some(ReviewAction::Drop),
                "d drop",
                Some("d drop"),
            ),
            KeyRow::new(&[], None, ":hide later", None),
            KeyRow::new(scroll, None, "j/k scroll", None),
        ])
    }

    /// **What `key` answers**, or `None` for a key that answers nothing
    /// here ([`key_table`](Self::key_table)) — plain keys only, as a call's.
    pub(crate) fn resolve(&self, key: &KeyEvent) -> Option<ReviewAction> {
        self.key_table().resolve(key)
    }

    /// **The legend, from what the review is now** — the one writer of
    /// `legend`, as `ToolPrompt::relegend` is of a call's.
    fn relegend(&mut self) {
        self.legend = Some(self.key_table().legend());
    }
}

impl App {
    /// **A reviewed draft, put to its session** — its decision, behind its
    /// calls, nothing taking the terminal: its pane opens beside the session
    /// at the event's end, its keys waiting, as a call's does
    /// (`settle_decisions`). In the same event as the distillation's end,
    /// before `settle`, so a prompt queued behind the distillation is held by
    /// its review from the moment it exists (design review DR-B #1).
    pub(super) fn review_arrives(&mut self, buffer: BufferId, work: Box<SummaryWork>) {
        let made_from = self.made_from(&work);
        let shown = self.scrub_shown(work.draft.trim());
        self.decisions
            .add_review(buffer, Review::new(work, made_from, &shown));
        // Where it waits, said (§4: "the notice says where the review will
        // wait").
        let reach = match self.review_reach(buffer) {
            Reach::Beside(None) => "its review waits in the pane beside it".to_string(),
            Reach::Beside(Some(tab)) => {
                format!("its review waits in the pane beside it, in tab {tab} — `{tab}gt`")
            }
            Reach::Decide => format!("its review waits — `:decide {}`", buffer.number()),
            Reach::AfterCalls => "its review comes up after its session's calls".to_string(),
        };
        self.notify_for(buffer, format!("summary written — {reach}"));
    }

    /// **How `buffer`'s review is reached now** — the one answer every
    /// sentence about a waiting review reads (its arrival, the hold's
    /// recovery). They said `:decide N` of a review behind calls still to
    /// come, and `:decide` then found nothing to go to (review stop 8, D 1).
    pub(super) fn review_reach(&self, buffer: BufferId) -> Reach {
        let shown = self.where_shown(buffer);
        match self.decisions.on_pane(buffer) {
            Some(super::decisions::OnPane::Review(_))
                if self.decisions.wants_a_pane(buffer) && shown != super::tabs::Shown::Nowhere =>
            {
                Reach::Beside(shown.tab())
            }
            Some(super::decisions::OnPane::Review(_)) => Reach::Decide,
            _ => Reach::AfterCalls,
        }
    }

    /// **Where a summary's review will wait, said at the press** (§4: "When
    /// the session isn't shown at the press, the notice says where the review
    /// will wait") — beside its session when a window shows it, else what
    /// brings it. It said "it opens for review when it is written", which
    /// was `$EDITOR` taking the terminal then.
    pub(crate) fn review_when_written(&self, buffer: BufferId) -> String {
        // Beside its session wherever a window shows it — this tab's first,
        // else the session's own tab, where a decision opens (Q1 (a)).
        match self.where_shown(buffer) {
            super::tabs::Shown::Here => {
                "its review opens beside the session when it is written".to_string()
            }
            super::tabs::Shown::InTab(tab) => {
                format!("its review opens beside the session, in tab {tab}, when it is written")
            }
            super::tabs::Shown::Nowhere => format!(
                "its review waits for you when it is written — `:decide {}`",
                buffer.number()
            ),
        }
    }

    /// **What a draft was made from, said** — `2 turns of b3 5da64e5a`: its
    /// session as `:ls` names it, or for a gathered summary that it was.
    fn made_from(&self, work: &SummaryWork) -> String {
        let n = work.sources.len();
        let turns = if n == 1 { "turn" } else { "turns" };
        match &work.target {
            SquashTarget::Range { episode, .. } => {
                format!("{n} {turns} of {}", self.session_tag(*episode))
            }
            SquashTarget::Branch { branch, .. } => {
                format!("{n} {turns} of {}", self.session_tag(*branch))
            }
            SquashTarget::Gather { .. } => format!("{n} {turns}, gathered"),
        }
    }

    /// **`$EDITOR` on what `buffer`'s pane shows** — the key pressed in it
    /// (`ToolKeyEffect::Edit`): a call's command, or a review's draft. One
    /// door from the event loop, dispatched by the kind (A2.1's seam).
    pub(crate) fn edit_decision(
        &mut self,
        buffer: BufferId,
        editor: &mut dyn crate::editor::Editor,
    ) {
        match self.decisions.on_pane(buffer) {
            Some(super::decisions::OnPane::Call(_)) => self.edit_tool_command(buffer, editor),
            Some(super::decisions::OnPane::Review(_)) => self.edit_review(buffer, editor),
            None => {}
        }
    }

    /// **A review in `$EDITOR`** — the instruction sent and the model's
    /// reply, as one editable exchange (`summary_seed`), so what lands in
    /// history is what happened, both sides editable before anything
    /// composes. **Save-to-apply**: `:wq` composes what is saved; quit
    /// without a save and the review stays, said — `:x` on a draft left as
    /// it was writes nothing, which no file can tell from `:q!`, so the
    /// draft is not dropped and not applied (it was asked, `ApplySummary`);
    /// a failed editor or `:cq` keeps it too, with why, and so does a save
    /// the compose refuses (`compose_review`). Paid work goes only by `d`
    /// (design review DR-B #9).
    pub(crate) fn edit_review(&mut self, buffer: BufferId, editor: &mut dyn crate::editor::Editor) {
        let Some(seed) = self.decisions.review(buffer).map(Review::seed) else {
            return;
        };
        match editor.edit_saved(&seed, crate::editor::Filetype::Markdown) {
            Ok(Some(text)) => {
                self.compose_review(buffer, Some(text));
            }
            Ok(None) => {
                let kept_edit = (self.decisions.review_mut(buffer)).is_some_and(|review| {
                    review.close_unsaved();
                    review.edit_refused()
                });
                // Your refused edit, kept, is what `<CR>` opens — and `a` is
                // then the model's draft, not "it as it was" (review stop 8,
                // F 2).
                let said = match kept_edit {
                    true => {
                        "summary closed without saving — your edit waits too: `<CR>` opens it \
                         again, `a` applies the model's draft instead, `d` drops it"
                    }
                    false => {
                        "summary closed without saving — it waits: `a` applies it as it was, \
                         `<CR>` opens it again, `d` drops it"
                    }
                };
                self.notify_for(buffer, said);
            }
            Err(e) => {
                tracing::warn!(error = ?e, "summary review's editor failed; the draft waits");
                self.notify_for(
                    buffer,
                    format!(
                        "summary review: the editor failed ({e}) — the draft waits; `<CR>` opens \
                         it again, `d` drops it"
                    ),
                );
            }
        }
    }

    /// **`buffer`'s review, composed** — `edited` as `$EDITOR` saved it, or
    /// as drafted (`a`) — through the tail `s` composes by, as asked.
    ///
    /// **Where you stand is the window it sits under** (design review DR-B
    /// #6): answered from its pane, the landing read the decision pane as
    /// where you are, and a pane that hosts nothing sent the result to the
    /// aim's window — the `:graph` you pressed `S` in, borrowing your only
    /// transcript, retired by it. So you are stood back in that window
    /// first, as closing a window in vim returns you to another.
    ///
    /// Its pane, left with nothing, is closed at the event's end
    /// (`settle_decisions`), as a call's is: placement is the one rule that
    /// closes a decision pane you did not.
    ///
    /// **Taken only once it composed** (review stop 8, A 1 / B 1): the
    /// compose refuses a session changed since, an edit that no longer
    /// parses — the editable form is JSON, so a newline typed in the draft's
    /// string is one — or a write that fails, and the review was taken
    /// first, so each threw the paid draft away and your edit with it. Kept,
    /// it says so beside the refusal, and you are stood back in its pane.
    pub(crate) fn compose_review(
        &mut self,
        buffer: BufferId,
        edited: Option<String>,
    ) -> Option<shannon_core::EpisodeId> {
        let pane = self.decision_pane_of(buffer);
        let review = self.decisions.review(buffer)?;
        // `a` after an edit the compose refused applies the model's draft:
        // said, or your edit went with no word (review stop 8, F 2).
        let drops_your_edit = edited.is_none() && review.edit_refused();
        let work = review.work();
        let (target, sources, seal) =
            (work.target.clone(), work.sources.clone(), work.seal.clone());
        let text = edited.clone().unwrap_or_else(|| Self::summary_seed(work));
        let host = pane
            .and_then(|p| self.window_of(p))
            .and_then(super::Window::decides)
            .map(|(_, host)| host);
        let stood_in_it = pane.is_some() && pane == Some(self.workspace.focused());
        if let (Some(host), true) = (host, stood_in_it) {
            self.focus_pane(host);
        }
        let graph = self.current_graph();
        let composed = self.commit_summary_into(buffer, graph, target, &sources, &text, Some(seal));
        if composed.is_some() {
            self.decisions.take_review(buffer);
            if drops_your_edit {
                self.notify_for(buffer, "the model's draft applied — your edit was not used");
            }
            return composed;
        }
        let again = match &edited {
            Some(saved) if !saved.trim().is_empty() => {
                "`<CR>` opens your edit again, `a` applies the model's draft instead"
            }
            _ => "`<CR>` opens it again",
        };
        if let Some(review) = self.decisions.review_mut(buffer)
            && let Some(saved) = edited
        {
            review.refuse_edit(saved);
        }
        if let (Some(pane), true) = (pane, stood_in_it) {
            self.focus_pane(pane);
        }
        self.notify_for(
            buffer,
            format!("the summary draft waits — {again}, `d` drops it"),
        );
        None
    }

    /// **`buffer`'s review, dropped** (`d`, `:q!`) — nothing composed, and
    /// said what. Its pane goes at the event's end, as `compose_review`'s.
    pub(crate) fn drop_review(&mut self, buffer: BufferId) {
        let Some(review) = self.decisions.take_review(buffer) else {
            return;
        };
        self.notify_for(
            buffer,
            format!(
                "dropped the summary draft of {} — nothing composed",
                review.made_from()
            ),
        );
    }
}

/// The draft is a decision's page: sized, folded and drawn as a call's is.
impl DecisionPage for Review {
    fn text(&self) -> &TextView {
        &self.text
    }

    fn text_mut(&mut self) -> &mut TextView {
        &mut self.text
    }

    fn legend(&self) -> Option<&PageLine> {
        self.legend.as_ref()
    }

    /// **The pane's title**: the question owed, what the editor last did
    /// with it, and what it was made from — the state ahead of the subject,
    /// as a call's title keeps what it must first: `S` is pressed in
    /// `:graph`, whose inspector halves the pane, and the state was the part
    /// cut (review stop 8, F 3). The same at any size: nothing in it is the
    /// body's to show.
    fn title(&self, _rows: u16, _width: u16) -> String {
        let since = match self.edited {
            Edited::Not => "",
            Edited::Unsaved => " · closed unsaved",
            Edited::Refused(_) => " · your edit not applied",
        };
        format!(" review this summary?{since} — {} ", self.made_from)
    }

    fn subject(&self) -> String {
        format!("the summary of {}", self.made_from)
    }

    fn keys_in_short(&self) -> String {
        self.key_table().in_short()
    }

    fn not_a_key(&self) -> String {
        self.key_table().not_a_key()
    }

    fn answers_width(&self) -> usize {
        self.key_table().answers_width()
    }
}
