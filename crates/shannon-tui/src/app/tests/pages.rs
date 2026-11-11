//! **Pages as buffers** (`buffers.md` §10, V3): the manual, `:analyze`'s
//! report and the notice history are one unlisted buffer each, numbered from
//! the sessions' counter; every window on one reads it; closing the last
//! keeps it, with where you were; `:ls!` names it and `:b N` returns to it.
//! A report replaced starts every window over; the history read again keeps
//! every reader's place.

use super::*;

fn k(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

/// A page window's lines, as text.
fn page_text(app: &App, pane: crate::wm::PaneId) -> Vec<String> {
    let window = app.pane_page(pane).expect("a page window");
    window.text().lines().iter().map(|l| l.text()).collect()
}

/// What a listing pane shows, as text.
fn listing_text(app: &App, pane: crate::wm::PaneId) -> String {
    app.pane_listing(pane)
        .expect("a listing")
        .lines()
        .iter()
        .map(|l| l.text())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The page buffers made, in number order.
fn pages(app: &App) -> Vec<&crate::page::PageBuffer> {
    app.buffers.pages().collect()
}

/// The help buffer's number, as `:ls!` gives it.
fn help_number(app: &App) -> u64 {
    let pages = pages(app);
    let [help] = pages.as_slice() else {
        panic!("one page, not {}", pages.len());
    };
    help.id().number()
}

/// **One manual, however many windows read it** — `:help`, a split of it,
/// and `:help <topic>` elsewhere all show the one buffer.
#[test]
fn every_help_window_reads_the_one_buffer() {
    let mut app = test_app();
    app.open_help_command(None);
    let first = app.workspace.focused();
    app.split_command("help", SplitDir::Vertical);
    let second = app.workspace.focused();
    app.split_command("output", SplitDir::Horizontal);
    app.open_help_command(Some("windows"));
    let third = app.workspace.focused();
    assert_eq!(pages(&app).len(), 1, "one help buffer");
    let help = pages(&app)[0].id();
    for pane in [first, second, third] {
        assert_eq!(
            app.pane_page(pane).map(|w| w.buffer()),
            Some(help),
            "{pane:?} shows it"
        );
    }
}

/// **A refused topic makes nothing** — no buffer, so no number taken from
/// the sessions' counter for a page nobody opened.
#[test]
fn a_refused_topic_makes_no_buffer() {
    let mut app = test_app();
    app.open_help_command(Some("nonsense"));
    assert!(pages(&app).is_empty(), "nothing made");
    assert!(app.notice().is_some_and(|n| n.contains("nonsense")));
}

/// **`:q` keeps the manual, and `:b N` goes back to where you were** —
/// the line you were on, in the middle of the window (vim's `'"`, as `:b`
/// restores it), not the top the tag jump opened at; the window titled for
/// the page, not the topic it was first opened at.
#[test]
fn closing_the_manual_keeps_it_and_b_returns_where_you_were() {
    let mut app = test_app();
    // From the transcript, so the split is a full-height column — split from
    // the prompt, it is the prompt bar's height, where `scrolloff` meets the
    // middle and the two placements cannot be told apart.
    app.set_focus(PaneKind::Output);
    app.split_command("output", SplitDir::Vertical);
    let pane = app.workspace.focused();
    app.open_help_command(Some("windows"));
    let anchor = crate::help_view::anchor("windows").expect("a chapter");
    for _ in 0..5 {
        app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Help), k('j'), (60, 20));
    }
    app.settle();
    app.dispatch_command_for_test("q");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Output),
        "premise"
    );

    let n = help_number(&app);
    let row = app
        .buffers_listing(true)
        .lines()
        .find(|l| l.trim_start().starts_with(&format!("b{n} ")))
        .expect("`:ls!` names it")
        .to_string();
    assert!(row.contains("uh"), "unlisted, hidden: {row}");
    assert!(row.contains(&format!("line {}", anchor + 6)), "{row}");

    app.dispatch_command_for_test(&format!("b{n}"));
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Help))
    );
    let frame = Rect::new(0, 0, 80, 40);
    app.before_frame(frame.width, frame.height);
    let (w, h) = app.pane_content_size(pane, frame);
    let window = app.pane_page(pane).expect("a window on it");
    assert_eq!(window.title(), " help ");
    let text = window.text();
    assert_eq!(text.cursor(), anchor + 5, "the line you were on");
    let row = text.rows_before(text.cursor(), w) - text.scroll() as usize;
    let h = h as usize;
    assert!(
        (h / 3..=h * 2 / 3).contains(&row),
        "in the middle of {h} rows, not at the top: row {row}"
    );
}

/// **A jump not yet drawn is where you were** — `:help windows` and `:q`
/// before a frame: only the jump's place in the window is held; the cursor
/// is on its line already (`TextView::reveal`).
#[test]
fn a_jump_not_yet_drawn_counts_where_it_lands() {
    let mut app = test_app();
    app.open_help_command(Some("windows"));
    app.settle();
    app.dispatch_command_for_test("q");
    assert_eq!(
        pages(&app)[0].left_at(),
        crate::help_view::anchor("windows")
    );
}

/// **The wheel moves a page you are not in, and that counts** — the
/// wheel drags an unfocused page's cursor (`wheel_scroll_at`), and you
/// were reading there.
#[test]
fn a_page_moved_by_the_wheel_is_where_you_were() {
    let mut app = test_app();
    let other = app.workspace.focused();
    app.split_command("output", SplitDir::Vertical);
    let help = app.workspace.focused();
    app.open_help_command(None);
    app.before_frame(80, 24);
    app.settle();
    assert_eq!(pages(&app)[0].left_at(), Some(0), "premise: at the top");
    app.focus_pane(other);
    for _ in 0..4 {
        app.wheel_scroll_at(help, false, (40, 20));
    }
    let moved = app.pane_page(help).expect("help").text().cursor();
    assert!(moved > 0, "premise: the wheel dragged the cursor");
    app.settle();
    assert_eq!(pages(&app)[0].left_at(), Some(moved));
}

/// **Two windows on the manual: the one you are in is the one kept** —
/// whichever comes later in the ring.
#[test]
fn of_two_windows_the_focused_one_is_kept() {
    let mut app = test_app();
    app.open_help_command(None);
    app.split_command("help", SplitDir::Vertical);
    let order: Vec<_> = app
        .workspace
        .ring_order()
        .into_iter()
        .filter(|&p| app.pane_page(p).is_some())
        .collect();
    let [earlier, later] = order[..] else {
        panic!("two help panes: {order:?}");
    };
    // The later one at the end of the manual, the earlier one three lines in,
    // and the earlier one focused.
    app.focus_pane(later);
    app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Help), k('G'), (60, 20));
    app.focus_pane(earlier);
    for _ in 0..3 {
        app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Help), k('j'), (60, 20));
    }
    app.settle();
    assert_eq!(pages(&app)[0].left_at(), Some(3));
}

/// **`:ls!` lists the manual in its place by number; `:ls` does not** — one
/// counter numbers sessions and pages, so the session opened after `:help`
/// is b3, and the listing says why.
#[test]
fn ls_bang_lists_pages_by_number_and_ls_does_not() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("first");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    app.open_help_command(None);
    app.dispatch_command_for_test("q");
    app.new_session_command();
    assert_eq!(help_number(&app), 2, "the manual took b2");
    assert_eq!(app.current.number(), 3, "so the next session is b3");

    let all = app.buffers_listing(true);
    let at = |n: u64| {
        all.lines()
            .position(|l| l.trim_start().starts_with(&format!("b{n} ")))
            .unwrap_or_else(|| panic!("no b{n} in\n{all}"))
    };
    assert!(at(1) < at(2) && at(2) < at(3), "by number:\n{all}");
    assert!(all.contains("the manual"), "{all}");
    let some = app.buffers_listing(false);
    assert!(
        !some.contains("the manual"),
        "`:ls` lists sessions:\n{some}"
    );
}

/// **`:ls` says a session under a viewer is hidden, and where** — `a` is a
/// pane drawing it, as vim's; a pane lent to a viewer draws the viewer. It
/// said `%a … pane 1` there, beside the 🔔 that says no pane drew it (Steven,
/// 10-23; review stop 9, F1); the lent pane is still named, under what.
#[test]
fn ls_says_a_session_under_a_viewer_is_hidden_and_where() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let hex = app.buffer().conversation.episode().id.to_hex();
    let row = |app: &App| {
        (app.buffers_listing(false).lines())
            .find(|l| l.contains(&hex[..8]))
            .expect("its row")
            .to_string()
    };
    let drawn = row(&app);
    assert!(drawn.contains("%a") && drawn.contains("pane 1"), "{drawn}");
    assert!(!drawn.contains("under"), "{drawn}");
    app.dispatch_command_for_test("ls");
    assert_eq!(
        app.focus(),
        PaneKind::Page(PageKind::Buffers),
        "premise: lent over its pane"
    );
    let lent = row(&app);
    assert!(lent.contains("%h"), "{lent}");
    assert!(lent.contains("pane 1 under ls"), "{lent}");
}

/// **A page's row keeps the mark column** — on by default, since the
/// shipped `unshown_session` marks — so its kind lines up under a session's
/// id; written without it, every `:ls!` page row sat the column's cells to
/// the left of the rows around it (review stop 9, A M4 and C 5: unpinned).
#[test]
fn a_pages_row_keeps_the_mark_column() {
    let mut app = test_app();
    app.open_help_command(None);
    app.dispatch_command_for_test("q");
    assert!(
        app.status_alerts().mark_cells() > 0,
        "premise: the column is on"
    );
    let all = app.buffers_listing(true);
    let row = |n: u64| {
        all.lines()
            .find(|l| l.trim_start().starts_with(&format!("b{n} ")))
            .unwrap_or_else(|| panic!("no b{n} in\n{all}"))
    };
    let cells_to = |row: &str, word: &str| {
        let at = row.find(word).unwrap_or_else(|| panic!("{word} in {row}"));
        crate::wrap::str_cells(&row[..at])
    };
    let hex = app.buffer().conversation.episode().id.to_hex();
    let kind = pages(&app)[0].kind().title();
    assert_eq!(
        cells_to(row(help_number(&app)), kind),
        cells_to(row(app.current.number()), &hex[..8]),
        "{all}"
    );
}

/// **The `!` survives being set aside** — under `:help` and given back by
/// `:q`, an `:ls!` is still `:ls!`; `:ls` typed there makes it `:ls`. The
/// bang was the window's until V5.1; it is the one `:ls` buffer's now
/// (`ls_bang_is_a_setting_of_the_one_listing`), which nothing here changes.
#[test]
fn ls_bang_survives_being_set_aside() {
    let mut app = test_app();
    app.open_help_command(None);
    app.dispatch_command_for_test("q");
    app.dispatch_command_for_test("ls!");
    let pane = app.workspace.focused();
    app.before_frame(100, 30);
    assert!(listing_text(&app, pane).contains("the manual"), "`:ls!`");

    app.open_help_command(None);
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Help)),
        "premise"
    );
    app.dispatch_command_for_test("q");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Buffers))
    );
    app.before_frame(100, 30);
    assert!(
        listing_text(&app, pane).contains("the manual"),
        "still `:ls!`:\n{}",
        listing_text(&app, pane)
    );

    app.dispatch_command_for_test("ls");
    app.before_frame(100, 30);
    assert!(!listing_text(&app, pane).contains("the manual"), "`:ls`");
}

/// **A session verb given a page's number says what it is** — not "there
/// is no b2" beside an `:ls!` that lists one.
#[test]
fn a_session_verb_given_a_page_says_what_it_is() {
    let mut app = test_app();
    app.open_help_command(None);
    let n = help_number(&app);
    app.dispatch_command_for_test(&format!("delete b{n}"));
    let notice = app.notice().expect("a refusal");
    assert!(
        notice.contains(&format!("b{n} is the manual, not a session")),
        "{notice}"
    );
    // The verbs that take a session's buffer say it alike (V5.4 (3),
    // `session_buffer_named`).
    for verb in ["resume", "decide"] {
        app.dispatch_command_for_test(&format!("{verb} b{n}"));
        let notice = app.notice().expect("a refusal");
        assert!(
            notice.contains(&format!("b{n} is the manual, not a session")),
            ":{verb}: {notice}"
        );
    }
}

/// **A session whose id reads as a buffer number** — `b` and four digits,
/// `b{n}` with `n >= 2`: `(its root episode, token, n)`. FOUND, not made:
/// `EpisodeId::from_seed` is deterministic, and about one id in a hundred
/// begins `b` and four digits.
fn an_id_reading_as_a_number() -> (shannon_core::Episode, String, u64) {
    let id = (0u32..)
        .map(|i| shannon_core::EpisodeId::from_seed(format!("page {i}").as_bytes()))
        .find(|id| {
            let hex = id.to_hex();
            hex.starts_with('b')
                && hex[1..5].chars().all(|c| c.is_ascii_digit())
                && hex[1..5].parse::<u64>().is_ok_and(|n| n >= 2)
        })
        .expect("the search is unbounded");
    let token = id.to_hex()[..5].to_string();
    let n: u64 = token[1..].parse().expect("digits");
    let root = shannon_core::Episode::root(
        id,
        shannon_core::TurnConfig {
            model: "stub".to_string(),
            max_tokens: None,
            effort: None,
        },
    );
    (root, token, n)
}

/// [`an_id_reading_as_a_number`]'s session, adopted into the first buffer:
/// `(app, token, n)`.
fn app_on_an_id_reading_as_a_number() -> (App, String, u64) {
    let (root, token, n) = an_id_reading_as_a_number();
    let mut app = test_app();
    let recall = app.unlock_turns(&[]).expect("nothing to unlock");
    app.adopt_session(
        app.current,
        root,
        Vec::new(),
        OnDisk::Absent,
        recall,
        Landing::Asked,
    );
    (app, token, n)
}

/// **A number that is a session's buffer AND begins that session's own id
/// is one reading** — `b{n}` names the buffer and, as an id prefix, the
/// session in it: shown, not refused as two (V5.4 (3), `readings`: one
/// session, however many ways it is read).
#[test]
fn b_on_a_session_number_its_own_id_begins_shows_it() {
    let (root, token, n) = an_id_reading_as_a_number();
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    // The session takes the number its id's digits read as.
    app.buffers.skip_to(n - 1);
    let recall = app.unlock_turns(&[]).expect("nothing to unlock");
    let held = app.buffer_holding(root, Vec::new(), OnDisk::Absent, recall, Landing::Asked);
    assert_eq!(
        held.number(),
        n,
        "premise: b{n} holds the session b{n}… names"
    );

    app.dispatch_command_for_test(&format!("b {token}"));
    assert_eq!(
        app.live_session(pane),
        Some(held),
        "shown: {:?}",
        app.notice()
    );
}

/// **`:b` on a page's number shows the page, though a session's id begins
/// so** — an id is typed from its first 8 characters (`id_prefix`), and
/// `b{n}` is five: the page's alone. At four it read as both and was refused
/// as two (V5.4 (3)); Steven, 11-10, took ids to 8. And a session verb reads
/// it so too — one reader: `:delete` is told it is the manual, never handed
/// the id's session past it.
#[test]
fn b_on_a_page_number_shows_the_page_never_an_id_it_begins() {
    let (mut app, token, n) = app_on_an_id_reading_as_a_number();
    // The manual takes the number the id's digits read as.
    app.buffers.skip_to(n - 1);
    app.open_help_command(None);
    app.dispatch_command_for_test("q");
    assert_eq!(help_number(&app), n, "premise: b{n} is the manual");
    let session = app.conversation().episode().id;
    assert!(
        session.to_hex().starts_with(&token),
        "premise: the session's id begins {token}"
    );

    let pane = app.workspace.focused();
    app.dispatch_command_for_test(&format!("b {token}"));
    assert!(
        matches!(app.workspace.kind_of(pane), Some(PaneKind::Page(_))),
        "the manual shown: {:?}",
        app.notice()
    );
    app.dispatch_command_for_test(&format!("delete {token}"));
    let notice = app.notice().expect("a refusal");
    assert!(
        notice.contains(&format!("b{n} is the manual, not a session")),
        ":delete: {notice}"
    );
    assert!(
        app.buffers
            .sessions()
            .any(|b| b.conversation.episode().id == session),
        "the id's session is not deleted"
    );
}

/// **`:b N` reads a buffer by its number, not through its episode** (V5.4
/// (3); the design review's M3): with two buffers on one episode — a state no
/// path is known to make, set up here — `:b 3` shows b3, and `:Send b3` is
/// b3's. Read through the episode, both found the FIRST buffer on it.
#[test]
fn b_n_names_a_buffer_by_its_number_not_its_episode() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let first = app.current;
    let episode = app.conversation().episode().clone();
    let recall = app.unlock_turns(&[]).expect("nothing to unlock");
    let second = app.buffer_holding(episode, Vec::new(), OnDisk::Absent, recall, Landing::Asked);
    let on = |app: &App, b| app.session_of(b).map(|s| s.conversation.episode().id);
    assert_ne!(second, first, "premise: a second buffer");
    assert_eq!(on(&app, second), on(&app, first), "premise: on one episode");
    assert_eq!(app.live_session(pane), Some(first), "premise: b1 shown");

    assert_eq!(
        app.send_target(&format!("b{}", second.number())),
        Ok(Named::Buffer(second)),
        "`:Send` names the buffer its number names"
    );
    app.dispatch_command_for_test(&format!("b {}", second.number()));
    assert_eq!(
        app.live_session(pane),
        Some(second),
        "`:b` shows the buffer its number names"
    );
}

/// **The same, when the number is a SESSION's** (V5.4 design review, HIGH
/// 1): `b{n}` numbers one session and begins another's id. At four
/// characters it named both, and `:b`, `:Send` and `:decide` refused it;
/// at 8 it is the buffer's alone, by every verb's one reader — `:b` shows
/// it, `:Send` aims there, and the other session is named by no verb.
#[test]
fn b_on_a_session_number_names_that_buffer_never_the_id_it_begins() {
    let (mut app, token, n) = app_on_an_id_reading_as_a_number();
    let other = app.current;
    // A turn, so `:new` leaves this session for a fresh one.
    app.conversation_mut().begin_turn("first");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    // The next session takes the number the other id's digits read as.
    app.buffers.skip_to(n - 1);
    app.new_session_command();
    assert_eq!(app.current.number(), n, "premise: b{n} is the new session");
    let shown = app.current;
    assert_ne!(shown, other, "premise: two sessions");

    app.notices.clear();
    app.dispatch_command_for_test(&format!("b {token}"));
    assert_eq!(app.current, shown, "b{n}'s: {:?}", app.notice());
    assert!(
        !app.notice().unwrap_or_default().contains("names 2"),
        "not two: {:?}",
        app.notice()
    );
    assert_eq!(
        app.send_target(&token),
        Ok(Named::Buffer(shown)),
        "`:Send` reads it as the buffer"
    );
}

// --- V3b: the report and the history ----------------------------------------

/// **One report, and every window on it takes the new one** (Steven, 10-20:
/// "`:analyze` is one buffer each command replaces") — `:analyze` from a
/// split replaces the report in the window beside it too, which starts over
/// at the top under the new command's title.
#[test]
fn a_new_report_reaches_every_window_on_it() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let first = app.workspace.focused();
    app.open_analyze_command("ls -la");
    app.split_command("analyze", SplitDir::Vertical);
    let second = app.workspace.focused();
    assert_ne!(first, second, "premise: two windows");
    app.focus_pane(first);
    app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Analyze), k('j'), (60, 20));
    assert_eq!(app.pane_page(first).unwrap().text().cursor(), 1, "premise");

    app.focus_pane(second);
    app.open_analyze_command("rm -r build");
    app.before_frame(100, 40);
    assert_eq!(
        app.buffers
            .pages()
            .filter(|p| p.kind() == PaneKind::Page(PageKind::Analyze))
            .count(),
        1,
        "one report buffer"
    );
    assert_eq!(
        page_text(&app, first),
        page_text(&app, second),
        "the new report"
    );
    let window = app.pane_page(first).unwrap();
    assert_eq!(window.title(), " analyze — rm -r build ");
    assert_eq!(window.text().cursor(), 0, "started over, at the top");
    assert!(
        app.buffers_listing(true).contains("rm -r build"),
        "`:ls!` names the report by its command"
    );
    // …and a session verb given its number says so, as a command.
    let n = app.pane_page(first).unwrap().buffer().number();
    app.dispatch_command_for_test(&format!("delete b{n}"));
    let notice = app.notice().expect("a refusal");
    assert!(
        notice.contains(&format!("b{n} is `:analyze rm -r build`, not a session")),
        "{notice}"
    );
}

/// **A new report forgets where you were in the old one** — `:b N` on it
/// opens at the top of the new report, not at a line of one that is gone.
#[test]
fn a_new_report_forgets_where_you_were_in_the_old() {
    let mut app = test_app();
    app.open_analyze_command("ls -la");
    for _ in 0..3 {
        app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Analyze), k('j'), (60, 20));
    }
    app.settle();
    let report = |app: &App| {
        app.buffers
            .pages()
            .find(|p| p.kind() == PaneKind::Page(PageKind::Analyze))
            .expect("the report")
            .left_at()
    };
    assert_eq!(report(&app), Some(3), "premise");
    app.dispatch_command_for_test("q");
    app.split_command("output", SplitDir::Vertical);
    app.open_analyze_command("rm -r build");
    assert_eq!(report(&app), None, "forgotten with the old report");
    app.dispatch_command_for_test("q");
    let pane = app.workspace.focused();
    let n = app
        .buffers
        .pages()
        .find(|p| p.kind() == PaneKind::Page(PageKind::Analyze))
        .expect("the report")
        .id()
        .number();
    app.dispatch_command_for_test(&format!("b{n}"));
    app.before_frame(100, 40);
    assert_eq!(app.pane_page(pane).unwrap().text().cursor(), 0, "its top");
}

/// **`:analyze` split with no report refuses, and makes nothing.**
#[test]
fn an_analyze_split_with_no_report_makes_no_buffer() {
    let mut app = test_app();
    app.split_command("analyze", SplitDir::Vertical);
    assert!(pages(&app).is_empty(), "nothing made");
    assert!(
        !app.workspace.has(PaneKind::Page(PageKind::Analyze)),
        "nothing opened"
    );
}

/// **The history read again keeps every other reader's place** — the same
/// document, newer: the window you read it in opens at the newest, and the
/// one beside it, scrolled back to read something, takes the newer lines
/// where it is.
#[test]
fn messages_read_again_keep_another_readers_place() {
    let mut app = test_app();
    for i in 0..30 {
        app.notify(format!("message {i}"));
    }
    app.set_focus(PaneKind::Output);
    let reader = app.workspace.focused();
    app.open_messages_command();
    // The `:messages` event's end: opened over the transcript you stand in,
    // focus moved, and the loop sees it here — unsettled, the first `g`'s
    // settle saw it and cancelled the `gg` it began.
    app.settle();
    for c in ['g', 'g', 'j', 'j', 'j'] {
        app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Messages), k(c), (60, 20));
    }
    assert_eq!(app.pane_page(reader).unwrap().text().cursor(), 3, "premise");

    app.split_command("output", SplitDir::Vertical);
    let other = app.workspace.focused();
    app.notify("said later");
    app.open_messages_command();
    app.before_frame(100, 40);
    assert_eq!(
        page_text(&app, reader),
        page_text(&app, other),
        "newer lines"
    );
    assert!(page_text(&app, reader).contains(&"said later".to_string()));
    assert_eq!(
        app.pane_page(reader).unwrap().text().cursor(),
        3,
        "kept its place"
    );
    let newest = page_text(&app, other).len() - 1;
    assert_eq!(
        app.pane_page(other).unwrap().text().cursor(),
        newest,
        "this one opened at the newest"
    );
}

/// The address of a window's lines — the same allocation means nothing
/// took them again (`TextView::set_lines` always makes a new one).
fn lines_at(text: &crate::text_view::TextView) -> *const crate::paint::PageLine {
    text.lines().as_ptr()
}

/// The pane of `kind`, by the layout.
fn pane_of(app: &App, kind: PaneKind) -> crate::wm::PaneId {
    app.workspace
        .rects(ratatui::layout::Rect::new(0, 0, 120, 40))
        .iter()
        .find(|r| r.kind == kind)
        .map(|r| r.id)
        .unwrap_or_else(|| panic!("a {kind:?} pane"))
}

/// **A frame with nothing new leaves every window's content where it is** —
/// a page window, one whose page was just read again, `:ls`, the inspector:
/// each takes its lines again only when they moved. Without the version and
/// compare checks, a window re-took its content every frame — 6.3 ms a frame
/// on the manual against a 0.5 ms frame — and every test passed (full
/// review 10-21, B 1 and C 6, mutated and measured).
#[test]
fn a_frame_with_nothing_new_leaves_every_window_untouched() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("first");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();

    app.open_help_command(None);
    let help = app.workspace.focused();
    app.before_frame(120, 40);
    let page = |app: &App, pane| lines_at(app.pane_page(pane).unwrap().text());
    let before = page(&app, help);
    app.before_frame(120, 40);
    assert_eq!(page(&app, help), before, "the manual, untouched");

    // A page read again — from another pane — takes the newer lines in the
    // reader's window once, then is left alone.
    app.split_command("output", SplitDir::Vertical);
    app.open_messages_command();
    let messages = app.workspace.focused();
    app.split_command("output", SplitDir::Vertical);
    app.notify("said later");
    app.open_messages_command();
    let stale = page(&app, messages);
    app.before_frame(120, 40);
    let synced = page(&app, messages);
    assert_ne!(synced, stale, "premise: the newer lines taken");
    app.before_frame(120, 40);
    assert_eq!(page(&app, messages), synced, "and then left alone");

    app.dispatch_command_for_test("ls");
    let listing = pane_of(&app, PaneKind::Page(PageKind::Buffers));
    app.before_frame(120, 40);
    let before = lines_at(app.pane_listing(listing).unwrap());
    app.before_frame(120, 40);
    assert_eq!(
        lines_at(app.pane_listing(listing).unwrap()),
        before,
        ":ls, untouched"
    );

    app.dispatch_command_for_test("graph");
    let inspector = pane_of(&app, PaneKind::Inspector);
    app.before_frame(120, 40);
    let before = lines_at(app.pane_inspector(inspector).unwrap());
    app.before_frame(120, 40);
    assert_eq!(
        lines_at(app.pane_inspector(inspector).unwrap()),
        before,
        "the inspector, untouched"
    );
}

/// **`:ls` counts a session's prompts once per change, not once a frame** —
/// the memo's hit path (`SessionBuffer::prompts`). Deleting it left the
/// listing right and every test green, and the scan of every session's
/// history back on every frame — 293 µs for 16 sessions of 3000 turns,
/// over a 250 µs frame (full review 10-21, D 2 on B 3's fix).
#[test]
fn ls_counts_prompts_once_per_change() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("first");
    app.conversation_mut().push_chunk("r");
    app.conversation_mut().commit_turn();
    app.dispatch_command_for_test("ls");
    app.before_frame(120, 40);
    let counted = crate::app::prompt_scans();
    app.before_frame(120, 40);
    app.before_frame(120, 40);
    assert_eq!(
        crate::app::prompt_scans(),
        counted,
        "unchanged: read, not counted"
    );
    // A change counts again: the memo is keyed by what it counts.
    app.conversation_mut().begin_turn("second");
    app.conversation_mut().push_chunk("r");
    app.conversation_mut().commit_turn();
    app.before_frame(120, 40);
    assert!(
        crate::app::prompt_scans() > counted,
        "a new prompt is counted"
    );
}

/// **Where you were is the window you read in** — not whichever window on
/// the page came last in the ring: two windows on the manual, read in the
/// earlier, the prompt stepped to, both closed: `:b N` returned to the
/// other's place (full review 10-21, A 11, probed).
#[test]
fn where_you_were_is_the_window_you_read_in() {
    let mut app = test_app();
    app.open_help_command(None);
    let one = app.workspace.focused();
    app.settle();
    app.split_command("help", SplitDir::Vertical);
    let other = app.workspace.focused();
    assert_ne!(one, other, "premise: two windows");
    app.settle();
    // Read in the window EARLIER in the ring: the ring-last one is the one
    // the old rule kept, so reading there could not tell them apart.
    let ring = app.workspace.ring_order();
    let at = |p| ring.iter().position(|&q| q == p).expect("in the ring");
    let (first, second) = if at(one) < at(other) {
        (one, other)
    } else {
        (other, one)
    };
    app.workspace.focus_pane(first);
    for _ in 0..10 {
        app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Help), k('j'), (60, 20));
    }
    app.settle();
    let read = app.pane_page(first).unwrap().text().cursor();
    assert!(read > 0, "premise: read down in the earlier");
    assert_ne!(
        app.pane_page(second).unwrap().text().cursor(),
        read,
        "premise: the later is elsewhere"
    );

    app.set_focus(PaneKind::Prompt);
    app.settle();
    assert_eq!(pages(&app)[0].left_at(), Some(read), "the window read in");
}

/// **`:sp` of a page window keeps your place in it**, as vim's `:split`
/// keeps the cursor line — it opened at the page's start (full review
/// 10-21, A 12, probed; docketed 10-21).
#[test]
fn a_split_of_a_page_keeps_your_place_in_it() {
    let mut app = test_app();
    app.open_help_command(None);
    let first = app.workspace.focused();
    for _ in 0..12 {
        app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Help), k('j'), (60, 20));
    }
    let here = app.pane_page(first).unwrap().text().cursor();
    assert!(here > 0, "premise: moved down");
    app.split_command("", SplitDir::Horizontal);
    let second = app.workspace.focused();
    assert_ne!(first, second, "premise: a new window");
    assert_eq!(app.pane_page(second).unwrap().text().cursor(), here);
}

/// **`:ls` counts a turn the moment it lands** — the count is kept per
/// session and taken again when its messages change, not scanned every
/// frame (full review 10-21, B 3); kept too long, it would read stale.
#[test]
fn ls_counts_a_turn_the_moment_it_lands() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("first");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    assert!(app.buffers_listing(false).contains("  1 turn "), "premise");
    app.conversation_mut().begin_turn("second");
    app.conversation_mut().push_chunk("another");
    app.conversation_mut().commit_turn();
    let listing = app.buffers_listing(false);
    assert!(listing.contains("  2 turns"), "{listing}");
}

/// **A page set aside is hidden** — `:ls!` names its pane only while a
/// window SHOWS it (`h`, no pane, under another view): set aside, `:b N`
/// brings it back, but no window draws it (V5.4 (2); `panes_on` counting
/// what a window holds aside passed every test).
#[test]
fn a_page_set_aside_is_listed_hidden() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.open_help_command(None);
    let n = help_number(&app);
    app.dispatch_command_for_test("messages");
    let row = (app.buffers_listing(true).lines())
        .find(|l| l.trim_start().starts_with(&format!("b{n} ")))
        .expect("`:ls!` names the manual")
        .to_string();
    assert!(
        row.contains("uh") && !row.contains("pane"),
        "set aside, not shown: {row}"
    );
}

/// **The session tree is a buffer too** (V4 identity) — numbered from the
/// one counter by the first tree and no other, listed by `:ls!` as a page is
/// (`u`; `a` while a pane shows it, `h` once none does), not by `:ls`; a
/// session verb given its number says what it numbers.
#[test]
fn the_session_tree_is_numbered_listed_and_named() {
    let mut app = test_app();
    // A row, not the header, which names what `u` marks.
    let named = |app: &App, all| {
        (app.buffers_listing(all).lines())
            .any(|l| l.trim_start().starts_with('b') && l.contains("the session tree"))
    };
    assert!(!named(&app, true), "no tree yet, no buffer");
    app.open_graph_command();
    let n = (app.buffers.trees().next())
        .expect("made by the first tree")
        .id()
        .number();
    assert_eq!(n, 2, "b1 the session, the tree b2");
    let row = |app: &App| {
        (app.buffers_listing(true).lines())
            .find(|l| l.trim_start().starts_with(&format!("b{n} ")))
            .map(str::to_string)
    };
    let shown = row(&app).expect("listed");
    assert!(shown.contains("ua") && shown.contains("graph"), "{shown}");
    assert!(shown.contains("the session tree"), "{shown}");
    assert!(!named(&app, false), "`:ls` lists sessions");

    app.dispatch_command_for_test("q");
    let kept = row(&app).expect("kept when its last window closed");
    assert!(kept.contains("uh"), "{kept}");
    app.split_command("graph", SplitDir::Vertical);
    let again = app.buffers.trees().next().map(|tree| tree.id().number());
    assert_eq!(
        again,
        Some(n),
        "the hidden tree taken back, as netrw takes a hidden listing back"
    );

    app.dispatch_command_for_test(&format!("clone b{n}"));
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains(&format!("b{n} is the session tree, not a session")),
        "{said}"
    );
}

/// **`:b N` opens the tree where you were in it** — the row you were on, as
/// a page's line (vim's `'"`), while `:graph` still opens on your session (and
/// so, at that event's end, is where you were). In a pane already showing a
/// tree it does nothing, as vim's `:b N` of the buffer the window shows: a
/// fresh tree there would drop its marks. Each command is settled, as the
/// event loop settles every event: unsettled, the test asserted a sequence
/// the program cannot produce (V4 verify, L4).
#[test]
fn b_n_opens_the_tree_where_you_were() {
    let (mut app, _, a, _, b, _) = two_buffers_on_a_store_refusing_deletes();
    assert_eq!(app.conversation().episode().id, b, "premise: in b");
    let on = |app: &App| app.graph_view().and_then(|tree| tree.selected_entry());
    let session = |e| Some(crate::graph_view::Entry::Node(Node::Episode(e)));
    let typed = |app: &mut App, command: &str| {
        app.dispatch_command_for_test(command);
    };
    app.open_graph_command();
    app.settle();
    let n = app.buffers.trees().next().unwrap().id().number();
    app.graph_view_mut().unwrap().reveal_and_select_episode(a);
    app.settle();
    typed(&mut app, "q");
    typed(&mut app, &format!("b{n}"));
    assert_eq!(on(&app), session(a), "`:b N` where you were");

    typed(&mut app, "q");
    typed(&mut app, "graph");
    let yours = on(&app);
    assert_ne!(yours, session(a), "`:graph` opens on your session");
    typed(&mut app, "q");
    typed(&mut app, &format!("b{n}"));
    assert_eq!(on(&app), yours, "and that is where you were last");

    app.graph_key_for_test(k(' '));
    let marked = app.graph_view().unwrap().marked_nodes();
    assert!(!marked.is_empty(), "premise: marked");
    app.dispatch_command_for_test(&format!("b{n}"));
    assert_eq!(
        app.graph_view().unwrap().marked_nodes(),
        marked,
        "in a tree, nothing"
    );
}

/// **`:b N` to a tree this window held brings that window back** — where
/// it stood in it, not where the tree was last stood in elsewhere (Steven,
/// 10-25, V4b Q2, for a view as for a session: vim's `:b` puts a window
/// back where it was in a buffer it showed). A window that never showed it
/// takes the tree's last place, mid-window (V4 (c)).
#[test]
fn b_n_to_a_tree_this_window_held_brings_it_back() {
    let (mut app, _, a, _, b, _) = two_buffers_on_a_store_refusing_deletes();
    let typed = |app: &mut App, command: &str| {
        app.dispatch_command_for_test(command);
    };
    let on = |app: &App, pane| app.pane_graph(pane).and_then(|t| t.selected_entry());
    let session = |e| Some(crate::graph_view::Entry::Node(Node::Episode(e)));
    app.set_focus(PaneKind::Output);
    let here = app.workspace.focused();
    typed(&mut app, "graph");
    app.graph_view_mut().unwrap().reveal_and_select_episode(b);
    app.settle();
    let n = app.buffers.trees().next().unwrap().id().number();
    app.graph_key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Output),
        "premise: b open"
    );

    // Another window on the tree, its cursor on a: the tree's last place.
    typed(&mut app, "vsplit");
    let there = app.workspace.focused();
    typed(&mut app, &format!("b{n}"));
    assert_eq!(
        app.workspace.kind_of(there),
        Some(PaneKind::Graph),
        "premise"
    );
    assert_eq!(
        on(&app, there),
        session(b),
        "a new window: the tree's last place"
    );
    app.graph_view_mut().unwrap().reveal_and_select_episode(a);
    app.settle();

    app.focus_pane(here);
    typed(&mut app, &format!("b{n}"));
    assert_eq!(
        on(&app, here),
        session(b),
        "where THIS window was, not the tree's last place"
    );
}

/// **`:b N` from the prompt bar leaves a tree lent over the aim's pane** —
/// it is the pane `:b N` would show the tree in (`host_pane`'s step 2), so the
/// tree is there already. Asked of the focused pane — the prompt, no tree — a
/// fresh tree replaced it, its marks gone (V4 verify, M2, probed).
#[test]
fn b_n_from_the_prompt_keeps_the_tree_lent_over_the_aim() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.open_graph_command();
    assert_eq!(
        app.workspace.focused(),
        pane,
        "premise: lent over the transcript"
    );
    app.graph_key_for_test(k(' '));
    let marked = app.pane_graph(pane).unwrap().marked_nodes();
    assert!(!marked.is_empty(), "premise: marked");
    app.set_focus(PaneKind::Prompt);
    let n = app.buffers.trees().next().unwrap().id().number();
    app.dispatch_command_for_test(&format!("b{n}"));
    assert_eq!(
        app.pane_graph(pane).map(GraphView::marked_nodes),
        Some(marked)
    );
}

// --- Listings are pages (V5.1) -----------------------------------------------

/// The page buffer of `kind`, if one was made.
fn page_of_kind(app: &App, kind: PaneKind) -> Option<&crate::page::PageBuffer> {
    app.buffers.pages().find(|p| p.kind() == kind)
}

/// Every pane showing `kind`, in ring order.
fn panes_of(app: &App, kind: PaneKind) -> Vec<crate::wm::PaneId> {
    (app.workspace.ring_order().into_iter())
        .filter(|&p| app.workspace.kind_of(p) == Some(kind))
        .collect()
}

/// **`:ls!` is a setting of the ONE `:ls` buffer** (V5.1; Steven, 10-25:
/// "the same list, showing more"), not each window's: with `:ls!` in one
/// pane and `:ls` typed in another, both windows are on the one buffer and
/// both list the sessions alone; `:ls!` typed again lists the pages in
/// both, and the title says `ls!` in both while it does — drawn, since
/// another pane's `:ls` changes it (Emacs's `*Buffer List*`, measured: one
/// buffer, its files-only flag on it).
#[test]
fn ls_bang_is_a_setting_of_the_one_listing() {
    let mut app = test_app();
    app.open_help_command(None);
    app.dispatch_command_for_test("q");
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("ls!");
    let one = app.workspace.focused();
    app.split_command("output", SplitDir::Vertical);
    app.dispatch_command_for_test("ls");
    let other = app.workspace.focused();
    assert_ne!(one, other, "premise: two windows");
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
    draw_frame(&mut app, &mut term);
    let on = |app: &App, pane| app.pane_page(pane).map(|w| w.buffer());
    assert!(on(&app, one).is_some(), "premise: a window on a page");
    assert_eq!(on(&app, one), on(&app, other), "one buffer");
    for pane in [one, other] {
        let text = page_text(&app, pane).join("\n");
        assert!(!text.contains("the manual"), "`:ls` cleared it:\n{text}");
    }
    let screen = rendered_text(term.backend().buffer());
    assert!(!screen.contains(" ls! "), "{screen}");

    app.dispatch_command_for_test("ls!");
    draw_frame(&mut app, &mut term);
    for pane in [one, other] {
        let text = page_text(&app, pane).join("\n");
        assert!(text.contains("the manual"), "`:ls!` set it:\n{text}");
    }
    let screen = rendered_text(term.backend().buffer());
    // The titles, by their corner: a row names the view by its title too
    // (`pane 1 under ls!`, review stop 10, F 8b).
    assert_eq!(
        screen.matches("┌ ls! ").count(),
        2,
        "both titles:\n{screen}"
    );
}

/// **A listing is derived once a frame, however many windows show it** —
/// its buffer's recipe run once (V5.1), where each pane held its own copy
/// and derived it, twice a frame for two `:ls` panes.
#[test]
fn a_listing_is_derived_once_a_frame_however_many_windows_show_it() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("ls");
    app.split_command("", SplitDir::Vertical);
    assert_eq!(
        panes_of(&app, PaneKind::Page(PageKind::Buffers)).len(),
        2,
        "premise: two windows"
    );
    app.before_frame(120, 40);
    let before = crate::app::listing_derives();
    app.before_frame(120, 40);
    assert_eq!(crate::app::listing_derives() - before, 1, "once, not twice");
}

/// **A listing is a buffer** (V5.1): numbered from the one counter when
/// first opened, listed `u` by `:ls!` as a page is (not by `:ls`), and
/// shown again by `:b N` — `:alias` here.
#[test]
fn a_listing_is_numbered_listed_and_shown_by_b_n() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.dispatch_command_for_test("alias");
    let n = (page_of_kind(&app, PaneKind::Page(PageKind::Aliases)))
        .expect("a buffer")
        .id()
        .number();
    assert!(n > app.current.number(), "numbered after the session: b{n}");
    app.dispatch_command_for_test("q");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Output),
        "premise"
    );
    let all = app.buffers_listing(true);
    let row = (all.lines())
        .find(|l| l.trim_start().starts_with(&format!("b{n} ")))
        .unwrap_or_else(|| panic!("`:ls!` lists b{n}:\n{all}"));
    assert!(row.contains("uh") && row.contains("the aliases"), "{row}");
    assert!(!app.buffers_listing(false).contains("the aliases"));
    app.dispatch_command_for_test(&format!("b{n}"));
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Aliases))
    );
    assert_eq!(app.pane_page(pane).map(|w| w.buffer().number()), Some(n));
}

/// **`:ls` typed in a window already on it keeps your place** — vim's `:b
/// N` of the buffer a window shows; a fresh window put the cursor back at
/// the top.
#[test]
fn ls_typed_again_in_its_window_keeps_your_place() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("ls");
    let pane = app.workspace.focused();
    app.before_frame(120, 40);
    app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Buffers), k('G'), (60, 20));
    let here = app.pane_page(pane).unwrap().text().cursor();
    assert!(here > 0, "premise: moved down");
    app.dispatch_command_for_test("ls");
    assert_eq!(app.workspace.focused(), pane, "premise: the same window");
    assert_eq!(app.pane_page(pane).unwrap().text().cursor(), here);
}

/// **`:sp` of a listing keeps your place in it**, as a page's does — the
/// listing's window started at its top when it was the pane's own content.
#[test]
fn a_split_of_a_listing_keeps_your_place_in_it() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("ls");
    let first = app.workspace.focused();
    app.before_frame(120, 40);
    app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Buffers), k('G'), (60, 20));
    let here = app.pane_page(first).unwrap().text().cursor();
    assert!(here > 0, "premise: moved down");
    app.split_command("", SplitDir::Horizontal);
    let second = app.workspace.focused();
    assert_ne!(first, second, "premise: a new window");
    assert_eq!(app.pane_page(second).unwrap().text().cursor(), here);
}

/// **`:themes` reads its places again each time it is typed — and only
/// then**: not each frame (its places are hundreds of files), not a split
/// of it. A theme put in yours since shows at the next `:themes`, in every
/// window on the list.
#[test]
fn themes_typed_again_reads_its_places_again() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("themes")).unwrap();
    let mut app = test_app();
    app.user_config = Ok(dir.path().join("config.toml"));
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("themes");
    let pane = app.workspace.focused();
    app.before_frame(120, 40);
    let listed = |app: &App| page_text(app, pane).iter().any(|l| l == "   fresh");
    std::fs::write(dir.path().join("themes/fresh.toml"), "").unwrap();
    app.before_frame(120, 40);
    assert!(!listed(&app), "not read again by a frame");
    app.split_command("", SplitDir::Horizontal);
    app.before_frame(120, 40);
    assert!(!listed(&app), "nor by a split");
    app.dispatch_command_for_test("themes");
    app.before_frame(120, 40);
    assert!(listed(&app), "read again by `:themes`");
}

// --- Rows carry ids (V5.2) ---------------------------------------------------

/// The line of `pane`'s page whose text begins `prefix`.
fn line_starting(app: &App, pane: crate::wm::PaneId, prefix: &str) -> usize {
    let page = page_text(app, pane);
    (page.iter())
        .position(|l| l.trim_start().starts_with(prefix))
        .unwrap_or_else(|| panic!("no `{prefix}` in {page:?}"))
}

/// `pane`'s page cursor put on line `at`.
fn put_cursor(app: &mut App, pane: crate::wm::PaneId, at: usize) {
    let window = app.window_of_mut(pane).expect("a pane");
    let page = window.page_mut().expect("a page window");
    page.text_mut().goto_line(at, 0);
}

/// `<CR>`, routed as production routes it.
fn enter(app: &mut App) {
    app.key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), (60, 20));
}

/// **`<CR>` on an `:ls` row shows that buffer HERE** (V5.2; Steven, 10-25:
/// rows carry ids, `<CR>` on a row — netrw's `<CR>`, Emacs's `RET`): by
/// `:b N`'s door, so `:ls` is the window's alternate and `<C-^>` comes back
/// to it, on the row you left.
#[test]
fn enter_on_an_ls_row_shows_that_buffer_here() {
    let mut app = test_app();
    // A turn, or `:new` reuses the blank session.
    app.conversation_mut().begin_turn("first");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    let first = app.current;
    app.new_session_command();
    assert_ne!(app.current, first, "premise: a second session");
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.dispatch_command_for_test("ls");
    app.before_frame(120, 40);
    let row = line_starting(&app, pane, &format!("b{} ", first.number()));
    put_cursor(&mut app, pane, row);
    enter(&mut app);
    assert_eq!(app.workspace.kind_of(pane), Some(PaneKind::Output));
    assert_eq!(app.pane_session(pane).id, first, "the row's buffer, here");
    press_ctrl_hat(&mut app, false);
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Buffers)),
        "`:ls` is the alternate"
    );
    assert_eq!(
        app.pane_page(pane).unwrap().text().cursor(),
        row,
        "on the row you left"
    );
}

/// **`<CR>` on a page's row in `:ls!` shows the page** — any buffer `:ls!`
/// lists, by the one door `:b N` ends in (`show_buffer`).
#[test]
fn enter_on_a_page_row_shows_the_page() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.open_help_command(None);
    app.dispatch_command_for_test("q");
    app.dispatch_command_for_test("ls!");
    app.before_frame(120, 40);
    let n = (page_of_kind(&app, PaneKind::Page(PageKind::Help)))
        .expect("the manual")
        .id()
        .number();
    let row = line_starting(&app, pane, &format!("b{n} "));
    put_cursor(&mut app, pane, row);
    enter(&mut app);
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Help))
    );
    assert_eq!(app.pane_page(pane).map(|w| w.buffer().number()), Some(n));
}

/// **`<CR>` on a line naming nothing says so** — the heading: the key is
/// claimed in `:ls`, and a silence would read as broken.
#[test]
fn enter_on_an_ls_line_naming_nothing_says_so() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.dispatch_command_for_test("ls");
    app.before_frame(120, 40);
    put_cursor(&mut app, pane, 0);
    enter(&mut app);
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("no buffer on this line")),
        "{:?}",
        app.notice()
    );
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Buffers))
    );
}

/// **A renewed listing keeps the cursor and the window's top on their
/// rows** (V5.2; V4 (a)'s rule for a tree, vim's `topline`) — two rules
/// added above the one you are reading in `:rules`: the cursor is on that
/// rule still, and the window's top on the line it was on. By index, each
/// was a line off: the session's `(none)` became two rules above them.
#[test]
fn a_renewed_listing_keeps_the_cursor_and_top_on_their_rows() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("rules");
    let pane = app.workspace.focused();
    let frame = Rect::new(0, 0, 120, 40);
    app.before_frame(frame.width, frame.height);
    // Far down, on a rule's line, so the window has scrolled.
    app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Rules), k('G'), (60, 20));
    for _ in 0..40 {
        if app.pane_page(pane).unwrap().row_under_cursor().is_some() {
            break;
        }
        app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Rules), k('k'), (60, 20));
    }
    app.before_frame(frame.width, frame.height);
    let (w, h) = app.pane_content_size(pane, frame);
    let seen = |app: &App| {
        let text = app.pane_page(pane).unwrap().text();
        let page = page_text(app, pane);
        let top = text.visible_rows(w, h as usize)[0].line;
        (page[text.cursor()].clone(), page[top].clone(), page.len())
    };
    let (reading, top, before) = seen(&app);
    assert!(
        app.pane_page(pane).unwrap().row_under_cursor().is_some(),
        "premise: on a rule"
    );
    assert!(
        app.pane_page(pane).unwrap().text().scroll() > 0,
        "premise: scrolled"
    );
    app.dispatch_command_for_test("allow echo *");
    app.dispatch_command_for_test("allow printf *");
    app.before_frame(frame.width, frame.height);
    let (now_reading, now_top, after) = seen(&app);
    assert!(
        after > before,
        "premise: lines came above: {before} → {after}"
    );
    assert_eq!(now_reading, reading, "the cursor on its rule");
    assert_eq!(now_top, top, "the window's top on its line");
}

/// **`:b N` of the page a window shows leaves it where it is** — vim's `:b
/// N` of the buffer the window shows, as a tree's does. It put a fresh window
/// in its place, at where you last were (the top, before a settle) rather
/// than where you are.
#[test]
fn b_n_of_the_page_a_window_shows_leaves_it_where_it_is() {
    let mut app = test_app();
    app.open_help_command(None);
    let pane = app.workspace.focused();
    for _ in 0..5 {
        app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Help), k('j'), (60, 20));
    }
    let here = app.pane_page(pane).unwrap().text().cursor();
    assert!(here > 0, "premise: moved down");
    app.dispatch_command_for_test(&format!("b{}", help_number(&app)));
    assert_eq!(app.pane_page(pane).unwrap().text().cursor(), here);
}

// --- Review stop 10 ---------------------------------------------------------

/// **From the prompt bar, `:b#` moves focus by what comes back** — a view
/// takes it (its keys are its own), a session leaves you in the prompt (you
/// type to it there), as `:b N` and `:help` typed there do. Which one the
/// window's history holds is not on screen, so the rule is said in
/// `*alternate*` (review stop 10, D 4: it was asked, and decided so —
/// the reviewer leaned to one rule for both).
#[test]
fn from_the_prompt_a_view_takes_the_focus_and_a_session_leaves_it() {
    let focus_after = |alternate_is_a_view: bool| {
        let mut app = test_app();
        app.set_focus(PaneKind::Output);
        let pane = app.workspace.focused();
        if alternate_is_a_view {
            app.dispatch_command_for_test("help");
            app.dispatch_command_for_test("b#");
        } else {
            app.conversation_mut().begin_turn("first");
            app.conversation_mut().push_chunk("a reply");
            app.conversation_mut().commit_turn();
            app.new_session_command();
        }
        app.settle();
        assert_eq!(
            app.workspace.kind_of(pane),
            Some(PaneKind::Output),
            "premise: a transcript"
        );
        app.set_focus(PaneKind::Prompt);
        app.settle();
        app.dispatch_command_for_test("b#");
        let shown = if alternate_is_a_view {
            PaneKind::Page(PageKind::Help)
        } else {
            PaneKind::Output
        };
        assert_eq!(
            app.workspace.kind_of(pane),
            Some(shown),
            "premise: the alternate came back in the aimed window"
        );
        (app.focus(), app.workspace.focused() == pane)
    };
    assert_eq!(
        focus_after(true),
        (PaneKind::Page(PageKind::Help), true),
        "a view takes it"
    );
    assert_eq!(
        focus_after(false),
        (PaneKind::Prompt, false),
        "a session leaves it"
    );
}

/// **`:open` on a page's number says it is the page**, though a session's
/// id begins so, as `:delete` does — it read the token as the session, past
/// the manual `:ls!` lists under that number (review stop 10, A 4). An id
/// is typed from 8 characters, so `b{n}` is the page's alone (11-10).
#[test]
fn open_on_a_page_number_says_it_is_the_page() {
    let (mut app, token, n) = app_on_an_id_reading_as_a_number();
    let session = app.conversation().episode().id;
    app.buffers.skip_to(n - 1);
    app.open_help_command(None);
    app.dispatch_command_for_test("q");
    assert_eq!(help_number(&app), n, "premise: b{n} is the manual");
    assert!(
        session.to_hex().starts_with(&token),
        "premise: an id begins {token}"
    );
    app.dispatch_command_for_test(&format!("open {token}"));
    assert!(
        app.notice()
            .is_some_and(|said| said.contains("is the manual, not a session or a turn")),
        "{:?}",
        app.notice()
    );
}

/// **A session shown for a command you typed is that buffer** — `:decide`
/// and `:tool` show the session they act on where no window does; with two
/// buffers on one episode (set up: no path is known to make them), the one
/// asked for, not the episode's first (`show_if_unseen`; review stop 10, A
/// 7: it went through the episode).
#[test]
fn a_session_shown_for_a_command_is_the_buffer_asked_for() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let episode = app.conversation().episode().clone();
    let recall = app.unlock_turns(&[]).expect("nothing to unlock");
    let second = app.buffer_holding(episode, Vec::new(), OnDisk::Absent, recall, Landing::Asked);
    assert!(
        app.panes_with_session(second, Over::ThisTab)
            .next()
            .is_none(),
        "premise: in no window"
    );
    app.show_if_unseen(second);
    assert!(
        app.panes_with_session(second, Over::ThisTab)
            .next()
            .is_some(),
        "the buffer asked for is shown"
    );
}

/// **`:open b{N}` shows buffer N**, as `:b N` does — by id or name it is the
/// episode's (`switch_session`), by number the buffer numbered; the arm went
/// through the episode before review stop 10 (A 4), so the second buffer on
/// one episode was never the one shown.
#[test]
fn open_by_a_number_shows_that_buffer() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let episode = app.conversation().episode().clone();
    let recall = app.unlock_turns(&[]).expect("nothing to unlock");
    let second = app.buffer_holding(episode, Vec::new(), OnDisk::Absent, recall, Landing::Asked);
    assert!(
        app.panes_with_session(second, Over::ThisTab)
            .next()
            .is_none(),
        "premise: in no window"
    );
    app.dispatch_command_for_test(&format!("open b{}", second.number()));
    assert!(
        app.panes_with_session(second, Over::ThisTab)
            .next()
            .is_some(),
        "the buffer numbered is shown: {:?}",
        app.notice()
    );
}

/// **`:open` on a page's number alone says what it numbers** — the same
/// sentence shape as beside an id, the token quoted (review stop 10, A 4).
#[test]
fn open_on_a_page_number_alone_names_the_page() {
    let mut app = test_app();
    app.open_help_command(None);
    let n = help_number(&app);
    app.dispatch_command_for_test("q");
    app.dispatch_command_for_test(&format!("open b{n}"));
    assert_eq!(
        app.notice(),
        Some(format!(
            "`:open`: `b{n}` is the manual, not a session or a turn"
        )),
    );
}

/// **`:ls` names a session's panes, not the windows it was LEFT in** — a
/// session `:new` left is held aside in its window (`:b#` brings it back),
/// but that window is not its: its row is `h`, no pane named (the column is
/// `panes_with_session`'s — shown, or beneath a view; review stop 10, B 2: one
/// walk for the whole listing, and counting what is aside passed every
/// test).
#[test]
fn ls_names_no_pane_for_a_session_left_aside() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let first = app.current;
    app.conversation_mut().begin_turn("first");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    app.new_session_command();
    let pane = app.workspace.focused();
    assert_eq!(held_buffers(&app, pane), [first], "premise: aside");
    let listing = app.buffers_listing(false);
    let row = (listing.lines())
        .find(|l| l.trim_start().starts_with(&format!("b{} ", first.number())))
        .expect("its row");
    assert!(row.contains(" h"), "hidden: {row}");
    assert!(!row.contains("pane"), "no pane is its: {row}");
}

/// **The close that leaves no transcript says so** — the prompt bar fills
/// the screen, aimed at a session no pane shows; it was silent, and the
/// manual said the second `:q` left (review stop 10, F 2, probe F6). `:close`
/// is the door now: the transcript's `:q` leaves, the last window beside the
/// bar (`App::last_window`). The bar's `:q` then leaves.
#[test]
fn closing_the_last_transcript_is_said() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let aim = app.prompt_target();
    assert!(
        !app.dispatch_command_for_test("close"),
        "premise: not leaving"
    );
    app.settle();
    assert!(
        !app.workspace.has(PaneKind::Output),
        "premise: no transcript"
    );
    assert_eq!(
        app.notice(),
        Some(format!(
            "the last transcript closed — b{n} is in no pane (`:b{n}` shows it; `:q` quits)",
            n = aim.number()
        ))
    );
    assert!(app.dispatch_command_for_test("q"), "the next `:q` leaves");
}

/// **CTRL-^ from the prompt bar names the window it acted on** — the aim's,
/// the lowest-numbered of two wearing the accent; "this window" from the
/// prompt bar named neither (review stop 10, F 3).
#[test]
fn ctrl_hat_from_the_prompt_names_the_aimed_pane() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let n = app.pane_number(pane).expect("premise: in the ring");
    app.set_focus(PaneKind::Prompt);
    app.dispatch_command_for_test("b#");
    assert_eq!(
        app.notice(),
        Some(format!(
            "no buffer to go back to in pane {n}, the aimed window"
        ))
    );
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("b#");
    assert_eq!(app.notice().as_deref(), Some(NO_ALTERNATE), "in the window");
}

/// **`<CR>` on a process `:ls` lists shows the session that started it** —
/// its line names that session (`from b1 …`); it answered "no buffer on
/// this line" (review stop 10, F 6, probe F9).
#[test]
fn enter_on_a_process_shows_the_session_that_started_it() {
    let (mut app, _exec) = app_with_a_leftover(0);
    let first = app.current;
    let episode = app.conversation().episode().id;
    app.leftover_sessions.insert("toolu_srv".into(), episode);
    app.set_focus(PaneKind::Output);
    // A turn, so `:new` leaves it rather than reusing it.
    app.conversation_mut().begin_turn("start the server");
    app.conversation_mut().push_chunk("started");
    app.conversation_mut().commit_turn();
    app.new_session_command();
    let pane = app.workspace.focused();
    assert_ne!(app.live_session(pane), Some(first), "premise: elsewhere");
    app.dispatch_command_for_test("ls");
    app.before_frame(160, 40);
    let row = line_starting(&app, pane, "python3");
    put_cursor(&mut app, pane, row);
    enter(&mut app);
    assert_eq!(
        app.live_session(pane),
        Some(first),
        "shown here: {:?}",
        app.notice()
    );
}

/// **`:ls` says what a session is doing right after its flags** — a narrow
/// window wrapped the last column onto a line of its own (review stop 10,
/// F 7, probe F7: 38 columns put `needs you` on a third line).
#[tokio::test]
async fn ls_says_what_a_session_is_doing_first() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.conversation_mut()
        .begin_turn("why is the build failing on main today");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    app.dispatch_command_for_test("tool rm /tmp/a");
    assert!(app.decision_holds(app.current), "premise: it needs you");
    let id = app.conversation().episode().id.short_hex();
    let listing = app.buffers_listing(false);
    let row = (listing.lines())
        .find(|l| l.contains(&id))
        .expect("its row");
    let (state, at) = (row.find("needs you"), row.find(&id));
    assert!(
        state.is_some() && state < at,
        "the state before the id: {row}"
    );
}

/// **`:b` alone does nothing, and says nothing** — vim's `:buffer` with no
/// argument stays on the buffer it is on; it quoted an empty token as "not
/// a session id" (review stop 10, F 8a).
#[test]
fn bare_b_stays_where_it_is() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let shown = app.live_session(pane);
    app.dispatch_command_for_test("b");
    assert_eq!(app.notice(), None);
    assert_eq!(app.live_session(pane), shown);
}

/// **`:ls!`'s row names the view over a session as its border does** —
/// `ls!`, not the kind's `ls` (review stop 10, F 8b, probe F3).
#[test]
fn ls_bang_names_the_view_over_a_session_as_its_border() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let n = app.pane_number(pane).expect("premise: in the ring");
    app.dispatch_command_for_test("ls!");
    let listing = app.buffers_listing(true);
    assert!(
        listing.contains(&format!("pane {n} under ls!")),
        "{listing}"
    );
    app.dispatch_command_for_test("ls");
    let listing = app.buffers_listing(false);
    assert!(
        (listing.lines()).any(|l| l.trim_end().ends_with(&format!("pane {n} under ls"))),
        "{listing}"
    );
}

/// **`:sg` from a tree is a fresh window on the same tree** — netrw's
/// `:Sexplore`, the directory opened again: its buffer, so its marks, but
/// a window of its own, on your session (`build_tree`), where a bare `:sp`
/// copies the window split
/// (`a_split_tree_is_a_second_window_on_it_its_marks_shared`). It took a
/// tree no window showed, else made one (review stop 10, F 4), until the
/// tree was one buffer (Steven, 10-26).
#[test]
fn sg_from_a_tree_is_a_fresh_window_on_the_same_tree() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let _second = add_persisted_session(&mut app, "Second");
    open_graph_pane(&mut app);
    let a = app.workspace.focused();
    let tree = app
        .pane_graph(a)
        .map(GraphView::tree)
        .expect("premise: a tree");
    let on = |app: &App, pane| app.pane_graph(pane).and_then(GraphView::selected_entry);
    let fresh = on(&app, a);
    for ch in ['j', 'm', 'f'] {
        app.graph_key_for_test(k(ch));
    }
    assert_ne!(on(&app, a), fresh, "premise: moved off your session");
    app.split_command("graph", SplitDir::Vertical);
    app.settle();
    let c = app.workspace.focused();
    assert_ne!(c, a, "premise: split");
    assert_eq!(
        app.pane_graph(c).map(GraphView::tree),
        Some(tree),
        "the same tree"
    );
    assert_eq!(
        app.pane_graph(c).map(|v| v.marked_nodes().len()),
        Some(1),
        "its marks"
    );
    assert_eq!(on(&app, c), fresh, "a window of its own, on your session");
    assert_eq!(app.buffers.trees().count(), 1, "no second tree");
}

/// **A buffer is found by its number in a handful of steps** — the list is
/// in number order, and a lookup is a binary search: at 1024 buffers, at
/// most 11 looked at, across a gap in the numbers too. It was a linear
/// scan, asked per session per ringing fact every frame — quadratic in the
/// buffers a run has opened (review stop 10, B 1). A count, not a time.
#[test]
fn a_buffer_is_found_by_its_number_in_log_steps() {
    let mut app = test_app();
    let mut ids = vec![app.current];
    for i in 0..1023u32 {
        if i == 500 {
            // A gap, as a test's `skip_to` makes one.
            app.buffers.skip_to(app.buffers.iter().count() as u64 + 7);
        }
        let episode = shannon_core::Episode::root(
            shannon_core::EpisodeId::from_seed(format!("lookup {i}").as_bytes()),
            shannon_core::TurnConfig {
                model: "stub".to_string(),
                max_tokens: None,
                effort: None,
            },
        );
        let recall = app.unlock_turns(&[]).expect("nothing to unlock");
        ids.push(app.buffer_holding(episode, Vec::new(), OnDisk::Absent, recall, Landing::Asked));
    }
    assert_eq!(
        app.buffers.sessions().count(),
        1024,
        "premise: 1024 buffers"
    );
    for &id in [ids[0], ids[499], ids[500], ids[501], ids[1023]].iter() {
        let before = crate::app::lookup_steps();
        assert!(
            app.session_of(id).is_some_and(|b| b.id == id),
            "{id:?} found"
        );
        let steps = crate::app::lookup_steps() - before;
        assert!(steps <= 11, "{id:?}: {steps} looked at");
    }
    // A number in the gap — opened before the 501st was added — is no buffer.
    assert!(ids[501].number() > ids[500].number() + 1, "premise: a gap");
    let gap = BufferId::numbered(ids[501].number() - 1).expect("a number");
    assert!(app.buffers.get(gap).is_none(), "{gap:?} is in the gap");
}

/// **`:ls` drops a first prompt's control characters**, as the transcript
/// does from the same text — an imported transcript's prompt can carry
/// escapes, and they went into the page raw, its columns counting them
/// (review stop 10, B 5).
#[test]
fn ls_drops_a_first_prompts_control_characters() {
    let mut app = test_app();
    app.conversation_mut()
        .begin_turn("\u{1b}[1mbold\u{7} and plain");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    let listing = app.buffers_listing(false);
    assert!(
        listing.contains("bold and plain"),
        "premise: listed: {listing}"
    );
    assert!(
        !listing.chars().any(|c| c.is_control() && c != '\n'),
        "no control characters: {listing:?}"
    );
}

/// **`:ls` names a session's panes in ring order** — `:vsp` puts the copy on
/// the LEFT, first in the ring and last made, and the column reads `pane
/// 1,2`, the order the corner numbers show (`panes_holding`'s sort; review
/// stop 10, C 2: deleting the sort passed every test, and the column read
/// `pane 2,1`).
#[test]
fn ls_names_a_sessions_panes_in_ring_order() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let made_first = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    let copy = app.workspace.focused();
    let ring = app.workspace.ring_order();
    assert_eq!(ring.first(), Some(&copy), "premise: the copy is ring-first");
    assert_eq!(
        ring.get(1),
        Some(&made_first),
        "premise: the first made, second"
    );
    assert_eq!(
        app.live_session(copy),
        app.live_session(made_first),
        "premise: one session"
    );
    let listing = app.buffers_listing(false);
    assert!(listing.contains("pane 1,2"), "{listing}");
}

/// **`:new` on the fresh session you are on changes nothing** — its
/// alternate kept, so `:b#` goes back to the session before it, not to
/// itself (`Window::enter`'s "already shows it"; review stop 10, A 1 and C 1:
/// the check stood in front of `:new` alone, and deleting it passed every
/// test).
#[test]
fn new_on_the_fresh_session_shown_keeps_its_alternate() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let first = app.current;
    app.conversation_mut().begin_turn("first");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    app.new_session_command();
    let fresh = app.live_session(pane).expect("a session");
    assert_ne!(fresh, first, "premise: a fresh session");
    assert_eq!(held_buffers(&app, pane), [first], "premise: the alternate");

    app.new_session_command();
    assert_eq!(app.live_session(pane), Some(fresh), "premise: stayed on it");
    assert_eq!(
        held_buffers(&app, pane),
        [first],
        "the history unchanged: {:?}",
        history_of(&app, pane)
    );
    app.dispatch_command_for_test("b#");
    assert_eq!(
        app.live_session(pane),
        Some(first),
        "`:b#` goes back to the first"
    );
}

/// **A writer key in a read-only pane says why, rather than doing nothing.**
///
/// The transcript and every viewer are the prompt's Normal mode minus the
/// writers, and "minus" was implemented as *fall through to the motion
/// resolver*, which answers `Ignored` for a non-motion. So `i`, `a`, `o`, `p`,
/// `x`, `D` and `Enter` were dead keys — while `h`, `l` and `e` ARE motions, so
/// typing a sentence into the transcript jittered the cursor and produced
/// nothing. Alive and broken at the same time, under a status row that
/// advertised `i:insert` (Reviewer F).
///
/// A fresh app per key on purpose: `post` collapses consecutive duplicates, so
/// two writers in one app would leave one notice and the second key would look
/// tested when it was not.
#[tokio::test]
async fn a_writer_key_in_a_read_only_pane_says_why() {
    // Every shape of writer the table knows: an Insert entry, an `o`-style
    // open, a register edit, the two single-key deletes, and an operator.
    let writers = [
        KeyCode::Char('i'),
        KeyCode::Char('a'),
        KeyCode::Char('o'),
        KeyCode::Char('p'),
        KeyCode::Char('x'),
        KeyCode::Char('D'),
        KeyCode::Char('d'),
        KeyCode::Char('u'),
    ];
    // **A pane's own verb is not a writer.** `:graph`'s `a`, `d` and `p` act
    // (`GraphVerb`, `viewer_claims_key`), and a decision's `a` and `d` answer
    // it (`OnPane::resolve`); every key a pane does not take reaches its
    // refusal — which is why the graph said nothing once: its own vocabulary
    // swallowed `i`, `o` and `x` (dogfooding).
    let own = |app: &App, code: KeyCode| {
        let key = KeyEvent::new(code, KeyModifiers::NONE);
        app.viewer_claims_key(&key)
            || (app.on_pane_in(app.workspace.focused()))
                .is_some_and(|on| on.resolve(&key).is_some())
    };
    // Every read-only kind, from the type: this named four of twelve
    // (review stop 12, T1 9).
    for kind in read_only_kinds() {
        // **Which voice refuses**: the keymap's "read-only", or — in a pane
        // deciding something — its legend, which names the keys that answer
        // and is said for every key it does not take (`handle_tool_key`:
        // "read-only" is true of `i` there, and says nothing of what the
        // pane waits for). Asked of the pane's state, not its kind: a decision pane
        // deciding nothing forwards its keys to the keymap.
        let mut quiet = read_only_pane(kind).await;
        let legend = quiet.decides_for(quiet.workspace.focused()).is_some();
        let voice = if legend {
            "not a decision key"
        } else {
            "read-only"
        };
        // **Premise, asserted rather than assumed**: a key with no binding at
        // all must stay silent — or, in a deciding pane, say only its legend.
        // Without this the gate passes on any app that notifies about
        // anything, and it would have passed against a build that shouted at
        // every keystroke ([[feedback_assert_the_subject]]).
        quiet.press_for_test(KeyEvent::new(KeyCode::F(12), KeyModifiers::NONE));
        assert!(
            match legend {
                true => quiet.notice().is_some_and(|n| n.contains(voice)),
                false => quiet.notice().is_none(),
            },
            "{kind:?}: premise — an unbound key must say nothing but the legend: {:?}",
            quiet.notice()
        );

        for code in writers {
            let mut app = read_only_pane(kind).await;
            // The rule is "a key that MEANS something must not vanish",
            // through the whole route. `i`, `o` and `x` are no pane's verb,
            // or a pane claiming them back would skip them here.
            if own(&app, code) {
                assert!(
                    !matches!(code, KeyCode::Char('i' | 'o' | 'x')),
                    "{kind:?}: premise — {code:?} is no pane's verb"
                );
                continue;
            }
            app.press_for_test(KeyEvent::new(code, KeyModifiers::NONE));
            let notice = app.notice();
            assert!(
                notice.as_deref().is_some_and(|n| n.contains(voice)),
                "{kind:?}: {code:?} is a writer and did not say {voice:?}: {notice:?}"
            );
            assert_eq!(
                app.focused_mode(),
                crate::keymap::Mode::Normal,
                "{kind:?}: {code:?} must not leave a read-only pane in another mode"
            );
        }

        // `Enter` is refused for a DIFFERENT reason and must say a different
        // thing: it is not a writer, it fires a billable turn composed in a
        // buffer you cannot see from here — except on a listing, whose rows
        // act on it (`focused_rows_act`: `:ls`, `:tabs`, `:themes`), which
        // the keymap-only seam this used never reached (Tier 3), or the
        // graph, whose `<CR>` is its own verb, or a review's, which it
        // answers. A call's has no `<CR>`: its legend names what does.
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let mut app = read_only_pane(kind).await;
        let rows_act = app.focused_rows_act() || own(&app, KeyCode::Enter);
        app.press_for_test(enter);
        let said = app.notice().unwrap_or_default();
        let names = if legend { voice } else { "sends" };
        assert!(
            !said.contains("read-only") && (rows_act || said.contains(names)),
            "{kind:?}: Enter must name the prompt, its legend, or its row: {said:?}"
        );
    }
}

/// **A read-only pane refuses a Visual writer too**, with the same voice.
///
/// `v` then `d` in the transcript: the selection is real and highlighted, so
/// only the delete is impossible — the one place in the app where a user has
/// visible evidence that their selection exists and then watches a key do
/// nothing to it.
#[tokio::test]
async fn a_visual_writer_in_the_transcript_says_why() {
    let mut app = read_only_pane(PaneKind::Output).await;
    app.press_for_test(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(
        app.focused_mode(),
        crate::keymap::Mode::Visual,
        "premise — the transcript CAN select, or `d` is refused for the wrong reason"
    );
    app.press_for_test(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(
        app.notice().is_some_and(|n| n.contains("read-only")),
        "a Visual `d` in the transcript said nothing: {:?}",
        app.notice()
    );
}

/// **`$readonly` describes the pane you are in** — `(read-only)` exactly
/// where no text can be typed, and not in the prompt; in Visual as in Normal.
///
/// It advertised `i:insert Esc:normal` everywhere except the decision pane —
/// because the fix for the decision pane was placed as an `if focus == Tool`
/// override rather than as a predicate, so it answered for the one member that
/// had been reported ([[feedback_placement_not_predicate]]). In the transcript
/// and every viewer the row went on promising Insert for a pane that has no
/// buffer to insert into. So the row's word is held to the refusal's, on the
/// rendered row.
///
/// It also had to name `^W`, the way out, since `:only` in a transcript once
/// ended the session — no prompt pane, no key to make one. That is closed
/// where it opened, `ensure_prompt` on every close
/// (`only_never_leaves_a_workspace_with_nowhere_to_type`), and `^W` moves
/// focus from every pane alike: saying so on the row told a vim hand nothing
/// and read as this pane's own key (Steven, 10-14).
#[tokio::test]
async fn readonly_is_said_where_no_text_can_be_typed() {
    use crate::keymap::Mode;
    let row = |app: &mut App| {
        screen(app, 160)
            .lines()
            .last()
            .unwrap_or_default()
            .to_string()
    };
    // **Every kind's row carries the variable**, so `$readonly` is under
    // test in each. The shipped rows say it only on the transcript and the
    // pages; the graph and the decision say it when you try to type, as
    // vim's E21 does (Steven, 10-27, review stop 13, S13-Q1).
    let carrying = || {
        // `$alerts` too: a row set writing none is said at load.
        let format = "'[$mode] {($readonly) }$pane{ · $alerts}'";
        let kinds = ["prompt", "output", "decision", "graph", "page"];
        let lines: Vec<String> = kinds.iter().map(|k| format!("{k} = {format}")).collect();
        shannon_core::Config::parse(&format!("[tui.status]\n{}\n", lines.join("\n"))).unwrap()
    };
    let pane = |kind| async move {
        let mut app = read_only_pane(kind).await;
        // Premise: every row took the format (what changed is what it set).
        let changed = app.adopt_config(carrying());
        assert!(
            changed.len() == 5 && changed.iter().all(|k| k.starts_with("tui.status.")),
            "{kind:?}: fixture — {changed:?}"
        );
        assert!(
            app.notice().is_none(),
            "{kind:?}: fixture — {:?}",
            app.notice()
        );
        app
    };
    let press =
        |app: &mut App, c| app.press_for_test(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    // **Every kind, both ways, against the keys.** Walking
    // `read_only_kinds()` asked the predicate for its own denominator: a kind
    // dropped from `is_read_only` left the loop and the loop stayed green
    // (review stop 12, Tier 3, by mutation). Whether `i` reaches Insert is
    // the witness, so the row's word must agree with what the key did.
    //
    // `:graph` disagreed until 10-27: its `i` said "this pane is read-only"
    // while its row left the word out, since its keys act (review stop 12,
    // Q12). The decision pane disagreed after that, in Visual: a writer
    // reached the keymap and was refused "read-only" under a row that did
    // not say it (review stop 13, A 1). Its text is closed in every state,
    // so it is read-only, whichever voice refuses.
    let (mut refused, mut selected) = (Vec::new(), Vec::new());
    for kind in PaneKind::all() {
        let mut app = pane(kind).await;
        let said = row(&mut app);
        press(&mut app, 'i');
        let notice = app.notice().unwrap_or_default();
        let types = app.focused_mode() == Mode::Insert;
        assert_eq!(
            said.contains("(read-only)"),
            !types,
            "{kind:?}: the row says {said:?}, and `i` said {notice:?}"
        );
        match types {
            // The prompt, and a page you may change (the tutor).
            true => assert!(
                said.starts_with(&format!(" [NORMAL] {}", kind.title())),
                "{kind:?}: {said}"
            ),
            false => {
                // Refused, and said why: the keymap's word, or the legend
                // of a pane deciding something, which names its keys.
                assert!(
                    notice.contains("read-only") || notice.contains("not a decision key"),
                    "{kind:?}: `i` refused in silence: {notice:?}"
                );
                refused.push(kind);
            }
        }
        // **And in Visual**, where a pane selects: a writer there is the
        // keymap's to refuse, in every pane.
        let mut app = pane(kind).await;
        press(&mut app, 'v');
        if types || app.focused_mode() != Mode::Visual {
            continue;
        }
        selected.push(kind);
        // The row as the writer meets it: the notice takes its line after.
        let said = row(&mut app);
        assert!(said.contains("[VISUAL] (read-only)"), "{kind:?}: {said}");
        press(&mut app, 'x');
        let notice = app.notice().unwrap_or_default();
        assert!(
            notice.contains("read-only") && app.focused_mode() != Mode::Insert,
            "{kind:?}: a Visual `x` must be refused as read-only: {notice:?}"
        );
    }
    assert_eq!(
        refused,
        read_only_kinds(),
        "the kinds that refuse a writer are the kinds the type calls read-only"
    );
    // Premise: the Visual half drove the pane A 1 found, and the transcript.
    for kind in [PaneKind::Tool, PaneKind::Output] {
        assert!(
            selected.contains(&kind),
            "{kind:?} did not select: {selected:?}"
        );
    }
}

/// **Every text page shows its cursor** — including the decision pane.
///
/// A pane that moves a cursor nobody can see is a pane whose `j` looks dead:
/// with the shipped `scrolloff = 5` the window does not move for five presses,
/// so five keys do nothing visible under a legend that says `j/k scroll`. Three
/// of the four pages shared `draw_text_pane` and got the highlight; the
/// decision pane builds its own `Paragraph` (pinned legend, fold marker) and so
/// inherited nothing — the `:analyze` defect one pane over (Reviewer A,
/// round 3).
///
/// Driven through the real renderer: the terminal's cursor inside THIS
/// pane's content, and no cell painted as one — a page painted its own
/// until every pane placed the terminal's (S21-Q23).
#[tokio::test]
async fn every_text_page_places_the_terminals_cursor() {
    use ratatui::style::Modifier;

    let (w, h) = (100u16, 44u16);
    // The decision pane and every text page, from the type: this named three
    // of eleven pages (review stop 12, T3 14).
    let kinds = PaneKind::all().into_iter();
    for kind in kinds.filter(|k| k.is_text_page() || *k == PaneKind::Tool) {
        let mut app = test_app();
        app.executor = Arc::new(shannon_core::RecordingExecutor::default());
        open_pane_of(&mut app, kind);
        assert!(
            app.workspace.has(kind),
            "premise: {kind:?} is on screen to be drawn"
        );
        app.clamp_text_panes(w, h);

        let (buf, at) = cursor_of(&mut app, w, h);
        // **Inside THIS pane's content** — not its border, whose title is
        // reversed when it has focus (S21-Q11), a signal of its own.
        let rect = app
            .workspace
            .frame_layout(Rect::new(0, 0, w, h))
            .pane(app.workspace.focused())
            .expect("the focused pane is on screen")
            .rect
            .inner(ratatui::layout::Margin::new(1, 1));
        assert!(
            at.is_some_and(|at| rect.contains(at)),
            "{kind:?}: the cursor at {at:?}, not in {rect:?} — `j` moves \
             something invisible"
        );
        let painted = (rect.y..rect.y + rect.height)
            .flat_map(|y| (rect.x..rect.x + rect.width).map(move |x| (x, y)))
            .filter(|&(x, y)| buf[(x, y)].modifier.contains(Modifier::REVERSED))
            .count();
        assert_eq!(painted, 0, "{kind:?}: a cell painted as a second cursor");
    }
}

/// **`:ls` counts Claude Code's deadline down** (design review DR-A #16;
/// Steven, 10-22: "The `:ls` timer is also good. We should implement it")
/// — what is left of the hour its hook waits, cut to the minute and then to
/// the second, never rounded up; with no pane showing the call, what brings
/// it. Each call's hook keeps its own clock, so the line says the soonest.
/// And a withdrawal at the deadline says it was the deadline — "stopped
/// waiting" alone read as Claude Code changing its mind — even one arriving
/// a moment before the deadline this app holds, which starts a moment
/// after the hook's.
#[tokio::test]
async fn ls_counts_claude_codes_deadline_down() {
    let (mut app, _provider) = provider_app();
    // No approval wait: going to the pane moves no time, and the count is
    // exact.
    std::sync::Arc::make_mut(&mut app.config)
        .tui
        .approval_delay_ms = 0;
    app.let_time_pass(Duration::ZERO);
    let rm = |n: u8| serde_json::json!({ "command": format!("rm /tmp/shannon-test-absent-{n}") });
    provider_asks(&mut app, "tu_1", "Bash", rm(1));
    let row = |app: &App| {
        let listing = app.buffers_listing(false);
        let line = listing.lines().find(|l| l.contains("needs you"));
        line.unwrap_or_default().trim_end().to_string()
    };
    assert!(
        row(&app).contains("needs you · 59m left"),
        "{:?}",
        row(&app)
    );
    // A second call ten minutes on: the first's deadline still comes first.
    app.let_time_pass(Duration::from_secs(600));
    provider_asks(&mut app, "tu_2", "Bash", rm(2));
    assert!(
        row(&app).contains("· 49m left"),
        "the soonest: {:?}",
        row(&app)
    );
    to_decision(&mut app);
    app.dispatch_command_for_test("hide");
    assert!(
        row(&app).contains("needs you — `:decide 1` · 49m left"),
        "hidden, what brings it: {:?}",
        row(&app)
    );
    app.let_time_pass(Duration::from_secs(61));
    assert!(row(&app).contains("· 47m left"), "cut: {:?}", row(&app));
    app.let_time_pass(Duration::from_secs(3540 - 600 - 61 - 30));
    assert!(row(&app).contains("· 30s left"), "{:?}", row(&app));

    app.let_time_pass(Duration::from_millis(29_500));
    app.feed_stream(StreamMsg::ToolWithdrawn(question("tu_1")));
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("refused — unanswered for 59 minutes, the most Claude Code waits"),
        "{said}"
    );
    assert_eq!(
        app.shown_call().map(|p| p.tool_use_id()),
        Some("tu_2"),
        "the next call, its own clock"
    );
    assert!(row(&app).contains("· 10m left"), "{:?}", row(&app));
}

/// **The deadline `:ls` counts down is the hook's, however late the app
/// reads it** (review stop 8, A 6): a question read five minutes after it
/// was asked — the loop held in `$EDITOR` meanwhile — has five minutes less
/// to run. It said the hour from when it was read.
#[tokio::test]
async fn ls_counts_the_hooks_deadline_not_when_it_was_read() {
    let (mut app, _provider) = provider_app();
    app.let_time_pass(Duration::ZERO);
    app.conversation_mut().begin_turn("go");
    let deadline = shannon_core::backend::claude_code::HOOK_DEADLINE;
    let answer_by = std::time::Instant::now()
        .checked_add(deadline - Duration::from_secs(300) + HOOK_READ_MARGIN);
    app.feed_stream(StreamMsg::ToolAsk(shannon_core::ToolQuestion {
        id: "tu_1".into(),
        name: "Bash".into(),
        input: serde_json::json!({"command": "rm /tmp/shannon-test-absent"}),
        cwd: here(&app),
        ask: question("tu_1"),
        answer_by,
    }));
    let listing = app.buffers_listing(false);
    let line = listing.lines().find(|l| l.contains("needs you"));
    assert!(line.is_some_and(|l| l.contains("· 54m left")), "{listing}");
}
