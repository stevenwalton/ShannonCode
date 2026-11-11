//! Notices: what is said, where, and for how long.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

// --- `:messages` ----------------------------------------------------------

/// Every path that reaches the notice row reaches the history, because both
/// writers of the slot go through `record_message`. A message the status row
/// cut in half and that `:messages` cannot show is the defect this whole thing
/// exists to remove.
#[test]
fn both_notice_doors_record_to_the_history() {
    let mut app = test_app();
    app.notify("a plain notice");
    app.refuse("a refusal");
    let seen: Vec<&str> = app.messages().collect();
    assert!(seen.contains(&"a plain notice"), "{seen:?}");
    assert!(seen.contains(&"a refusal"), "{seen:?}");
    assert_eq!(
        seen.last(),
        Some(&"a refusal"),
        "newest last, like vim: {seen:?}"
    );
}

/// **A notice shows what would reorder it, and every language as written**
/// (review stop 9, E2) — a notice quoting a name with a bidi override in it
/// says the override as its escape on the row and in `:messages`, where a
/// cell would draw the name silently reversed or short; Korean, Chinese, an
/// accent and Persian's non-joiner are as they were.
#[test]
fn a_notice_shows_what_would_reorder_it_and_every_language() {
    let mut app = test_app();
    app.notify("opened \u{202e}txt.exe");
    assert_eq!(app.notice().as_deref(), Some("opened \\u{202e}txt.exe"));
    assert!(
        app.messages().any(|m| m == "opened \\u{202e}txt.exe"),
        "the history the same"
    );
    for text in ["인증 수정", "修复认证", "cafe\u{301}", "می\u{200C}خواهم"] {
        app.notices.clear();
        app.refuse(text);
        assert_eq!(app.notice().as_deref(), Some(text), "as written");
    }
}

/// A held key on a refused motion re-reports the same refusal every frame. N
/// identical lines are worse than one at finding the message you came for.
#[test]
fn consecutive_duplicate_messages_collapse() {
    let mut app = test_app();
    app.refuse("nope");
    app.refuse("nope");
    app.notify("something else");
    app.refuse("nope");
    let seen: Vec<&str> = app.messages().collect();
    assert_eq!(
        seen,
        vec!["nope", "something else", "nope"],
        "only CONSECUTIVE repeats collapse — the later one is a new event"
    );
}

/// The history is bounded, because it grows with every keypress that refuses
/// something and nothing else ever trims it.
#[test]
fn the_history_is_bounded_and_drops_the_oldest() {
    let mut app = test_app();
    for i in 0..crate::app::MESSAGE_HISTORY + 50 {
        app.notify(format!("message {i}"));
    }
    assert_eq!(app.messages().len(), crate::app::MESSAGE_HISTORY);
    assert_eq!(
        app.messages().next(),
        Some("message 50"),
        "the oldest went, not the newest"
    );
}

/// **The row's notices are bounded too, between two keypresses** — a notify
/// in a loop would grow them without limit, and `notice()` joins them every
/// frame. Pinned by nothing (full review 10-21, B 4: deleting the bound
/// passed every test).
#[test]
fn the_rows_notices_are_bounded_and_keep_the_newest() {
    let mut app = test_app();
    for i in 0..crate::app::NOTICE_SLOT_MAX + 5 {
        app.notify(format!("notice {i}"));
    }
    assert_eq!(app.notices.len(), crate::app::NOTICE_SLOT_MAX);
    let last = format!("notice {}", crate::app::NOTICE_SLOT_MAX + 4);
    assert!(
        app.notices.last().is_some_and(|(_, n)| *n == last),
        "the newest kept: {:?}",
        app.notices
    );
}

/// `:messages` renders the history into a pane, newest last and scrolled to
/// it — the message you just half-read is the one you came for.
#[test]
fn messages_opens_a_pane_holding_the_history() {
    let mut app = test_app();
    app.notify("the first thing");
    app.notify("the second thing");
    app.open_messages_command();
    assert_eq!(app.focus(), PaneKind::Page(PageKind::Messages));
    let pane = app.workspace.focused();
    let view = app
        .pane_page(pane)
        .expect("content, not a blank pane")
        .text();
    let rendered: Vec<String> = view.lines().iter().map(|l| l.text()).collect();
    assert_eq!(rendered, vec!["the first thing", "the second thing"]);
}

/// Re-running it re-snapshots. A `:messages` pane is a still, so the verb has
/// to be what refreshes it — otherwise the pane you opened to read a cut-off
/// message goes stale the moment anything else is said.
#[test]
fn reopening_messages_picks_up_what_was_said_since() {
    let mut app = test_app();
    app.notify("before");
    app.open_messages_command();
    app.notify("after");
    app.open_messages_command();
    let pane = app.workspace.focused();
    let view = app.pane_page(pane).expect("content").text();
    assert_eq!(view.lines().len(), 2, "picked up `after`");
}

/// A viewer that opens blank reads as broken. Say which it is.
#[test]
fn an_empty_history_says_so_rather_than_opening_blank() {
    let mut app = test_app();
    app.open_messages_command();
    let pane = app.workspace.focused();
    let view = app.pane_page(pane).expect("content").text();
    assert_eq!(view.lines().len(), 1);
    assert_eq!(view.lines()[0].text(), "no messages yet");
}

/// The keys the status row advertises for a messages pane must actually be
/// taken by it. A viewer that claims a key and drops it is a dead key, and the
/// `Some(_) => {}` arm in `dispatch_viewer_key` is exactly how one gets there.
#[test]
fn a_messages_pane_is_a_viewer_that_takes_its_own_keys() {
    let mut app = test_app();
    app.notify("something to read");
    app.open_messages_command();
    assert_eq!(
        app.focused_viewer(),
        Some(PaneKind::Page(PageKind::Messages)),
        "advertises a legend and claims plain keys"
    );
}

/// **Two things said in one moment are both shown.**
///
/// The notice slot was an `Option<String>` written by 151 sites, last-write-
/// wins — so whenever two things reached it before the next frame, the second
/// erased the first, and the second was never the more useful one:
///
/// - `max_tokens` with no text said "the reply hit its length limit —
///   `:continue` resumes it" and then "billed 4096 and showed almost none of
///   it". The user got the diagnostic and not the answer.
/// - A job's ending was overwritten by the generic "child … finished" — which
///   I introduced *in the fix* for the one above.
/// - `stop_work` had "could NOT be saved to disk" replaced by a success line.
///
/// Three local fixes for one shape, the third inside the second. The shape was
/// the bug: the moment boundary already existed and there was never a reason
/// for the second write to destroy the first
/// (`[[feedback_features_split_identities]]`).
#[tokio::test]
async fn everything_said_in_one_moment_survives_it() {
    let mut app = test_app();
    app.submit("write something long");
    app.feed_stream(StreamMsg::Usage(Usage {
        input_tokens: 100,
        output_tokens: 4096,
        ..Default::default()
    }));
    // Two different events, one moment: a turn ends AND a queued prompt is
    // dropped because its session went away.
    app.feed_stream(StreamMsg::Stopped(shannon_core::StopReason::MaxTokens));
    app.feed_stream(StreamMsg::Complete);
    app.notify("something else happened too");

    let shown = app.notice().expect("both were said");
    assert!(
        shown.contains(":continue"),
        "the first is still there: {shown:?}"
    );
    assert!(
        shown.contains("something else happened too"),
        "…and so is the second: {shown:?}"
    );
    // Chronological, because the earlier one is the one that used to be lost.
    assert!(
        shown.find(":continue") < shown.find("something else"),
        "oldest first: {shown:?}"
    );
}

/// **A keypress begins a new moment; a key RELEASE does not.**
///
/// Both halves of `begin_moment`, and neither had a test — six tests simulated
/// the rule by clearing the slot themselves and none asserted it. The release
/// half has a bug history (a key-up wiping a message you had not read) and was
/// being held correct by a comment, which is this project's most expensive kind
/// of guard (`[[feedback_mutation_check_guards]]`).
///
/// It was untestable until `begin_moment` was split out: `handle_event` takes a
/// `ShannonTerminal`, which is a real stdout.
#[test]
fn a_keypress_begins_a_new_moment_and_a_release_does_not() {
    use crossterm::event::{Event, KeyEvent, KeyEventKind};

    let mut app = test_app();
    app.notify("something you have not read yet");
    assert!(app.notice().is_some(), "premise");

    // A release: not an action, and it must not take the message with it.
    let mut up = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    up.kind = KeyEventKind::Release;
    assert!(
        !app.begin_moment(&Event::Key(up)),
        "a release is not an event we act on"
    );
    assert!(
        app.notice().is_some(),
        "…and it left the unread message alone"
    );

    // A resize is not a user action either — it must not eat a refusal.
    assert!(app.begin_moment(&Event::Resize(80, 24)));
    assert!(app.notice().is_some(), "a resize is not an acknowledgement");

    // A press is the next moment.
    let down = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    assert!(app.begin_moment(&Event::Key(down)));
    assert!(
        app.notice().is_none(),
        "the message showed until you pressed a key, which is vim's rule"
    );
}
