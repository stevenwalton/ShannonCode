//! **What a close would take** — the work held by the panes it closes: a
//! request waiting in a decision pane, and a draft in a prompt (Steven,
//! 10-26: "we don't close and say something like 'Pending request in b#'";
//! a draft is each prompt window's own since the same day, so a close that
//! takes its window takes it). A close over either is refused without its
//! `!`, as vim refuses `:q` over a modified buffer (E37); forced, a request
//! waits, hidden, and a draft is dropped — said.

use crate::wm::BufferId;

/// **How a close was asked for** — what it may close over
/// (`App::close_pane_command`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Asked {
    /// `:q`, `:close`, `<C-w>c`, `<C-w>q` — over nothing held.
    Plain,
    /// `:hide` — keeps what it closes: a request waits, hidden; a draft has
    /// nowhere to wait, so a prompt holding one is not closed.
    Hide,
    /// `:q!` — over anything: a request waits, hidden; a draft is dropped.
    Bang,
}

/// **A prompt you would close over its draft** — said to `:q`, `:close`,
/// `<C-w>c`, `<C-w>q` and `:hide` on it.
pub(crate) const DRAFT_NOT_CLOSED: &str =
    "not closed — this prompt holds a draft: send or clear it, or `:q!` drops it";

/// **A request a close would take** — whose, and whether it is a call or a
/// summary's review, said as each (review stop 11, F 13: a review is not a
/// "request" you "answer"; its own refusal speaks of the draft).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) buffer: BufferId,
    pub(crate) review: bool,
}

/// **The work in some panes** (`App::held_in`): the requests waiting in
/// them, in number order, and the prompts among them holding a draft, by
/// their corner numbers (review stop 11, F 11: "a prompt holds a draft"
/// named none).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct HeldWork {
    pub(crate) requests: Vec<Request>,
    pub(crate) drafts: Vec<usize>,
}

impl HeldWork {
    /// Nothing a close would take.
    pub(crate) fn is_empty(&self) -> bool {
        self.requests.is_empty() && self.drafts.is_empty()
    }

    /// **Why a close is refused over it** — the remedy FIRST, `anyway` (the
    /// spelling that closes regardless) and what it does to each, then what
    /// holds ([`holds`](Self::holds)): an 80-column row cuts the tail, and
    /// cut the `!` when it came last (review stop 11, F 4; `App::leaving`
    /// learned this once). The one wording of every such refusal: a chat a
    /// request's pane sits under, `:only`, `:tabclose`, `:tabonly`.
    pub(crate) fn why(&self, anyway: &str) -> String {
        format!("{}; {}", self.remedy(anyway), self.holds())
    }

    /// **`anyway` and what it does** — "`:only!` closes anyway (it waits,
    /// hidden)": a request waits, hidden; a draft is dropped.
    pub(crate) fn remedy(&self, anyway: &str) -> String {
        let (requests, drafts) = (self.requests.len(), self.drafts.len());
        let does = match (requests, drafts) {
            (1, 0) => "it waits, hidden",
            (_, 0) => "they wait, hidden",
            (0, 1) => "it is dropped",
            (0, _) => "they are dropped",
            (1, 1) => "the request waits, hidden; the draft is dropped",
            _ => "requests wait, hidden; drafts are dropped",
        };
        format!("{anyway} closes anyway ({does})")
    }

    /// **What holds** — "b3's request waits in its pane", "b5's summary
    /// review…", "pane 3 holds a draft", or both.
    pub(crate) fn holds(&self) -> String {
        let names: Vec<String> = (self.requests.iter())
            .map(|r| {
                let what = if r.review {
                    "summary review"
                } else {
                    "request"
                };
                format!("b{}'s {what}", r.buffer.number())
            })
            .collect();
        let requests = match names.as_slice() {
            [] => None,
            [one] => Some(format!("{one} waits in its pane")),
            many => Some(format!(
                "{} wait in their panes",
                crate::trust::listed(many)
            )),
        };
        let panes: Vec<String> = self.drafts.iter().map(usize::to_string).collect();
        let drafts = match panes.as_slice() {
            [] => None,
            [one] => Some(format!("pane {one} holds a draft")),
            many => Some(format!("panes {} hold drafts", crate::trust::listed(many))),
        };
        match (requests, drafts) {
            (Some(requests), Some(drafts)) => format!("{requests}, and {drafts}"),
            (Some(said), None) | (None, Some(said)) => said,
            (None, None) => String::new(),
        }
    }
}

/// **What a forced close did with `drafts` drafts**, for its notice —
/// `None` when it dropped none: what `!` threw away is named, never gone in
/// silence.
pub(crate) fn dropped(drafts: usize) -> Option<String> {
    match drafts {
        0 => None,
        1 => Some("a draft dropped".to_string()),
        n => Some(format!("{n} drafts dropped")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Calls of `calls`, a review of `review`, drafts in `drafts`' panes.
    fn held(calls: &[u64], review: Option<u64>, drafts: &[usize]) -> HeldWork {
        let call = |n: u64| Request {
            buffer: BufferId(n),
            review: false,
        };
        let mut requests: Vec<Request> = calls.iter().map(|&n| call(n)).collect();
        requests.extend(review.map(|n| Request {
            buffer: BufferId(n),
            review: true,
        }));
        HeldWork {
            requests,
            drafts: drafts.to_vec(),
        }
    }

    /// **Each shape of the refusal: the remedy first, then what holds** —
    /// one and many, calls, a review, drafts by pane, and both; nothing held
    /// says nothing.
    #[test]
    fn why_says_the_remedy_then_what_holds() {
        let bang = "`:only!`";
        let b = |n: u64| BufferId(n).number();
        assert_eq!(
            held(&[3], None, &[]).why(bang),
            format!(
                "`:only!` closes anyway (it waits, hidden); b{}'s request waits in its pane",
                b(3)
            )
        );
        assert_eq!(
            held(&[3], Some(5), &[]).why(bang),
            format!(
                "`:only!` closes anyway (they wait, hidden); b{}'s request and b{}'s summary \
                 review wait in their panes",
                b(3),
                b(5)
            )
        );
        assert_eq!(
            held(&[], None, &[2]).why(bang),
            "`:only!` closes anyway (it is dropped); pane 2 holds a draft"
        );
        assert_eq!(
            held(&[], None, &[2, 4]).why(bang),
            "`:only!` closes anyway (they are dropped); panes 2 and 4 hold drafts"
        );
        assert_eq!(
            held(&[3], None, &[2]).why(bang),
            format!(
                "`:only!` closes anyway (the request waits, hidden; the draft is dropped); \
                 b{}'s request waits in its pane, and pane 2 holds a draft",
                b(3)
            )
        );
        // Many of both, the last arm, and one dropped: unasserted until review
        // stop 12 (T7).
        assert_eq!(
            held(&[3], Some(5), &[2, 4]).remedy(bang),
            "`:only!` closes anyway (requests wait, hidden; drafts are dropped)"
        );
        assert!(held(&[], None, &[]).is_empty());
        assert_eq!(dropped(0), None);
        assert_eq!(dropped(1).as_deref(), Some("a draft dropped"));
        assert_eq!(dropped(2).as_deref(), Some("2 drafts dropped"));
    }
}
