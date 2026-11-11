//! `shannon tutor` (`crate::tutor`, `tutor.md`): its lessons a page you may
//! change (`PageKind::modifiable`), edited through the page's one editor
//! (`PageBuffer::edit`), never sent, and its config no file of yours.

use super::*;

/// The app `shannon tutor` opens: launched on the lessons (`Opens::Tutor`,
/// `b1` held for them), in the window the prompt aims at, focused — and
/// the pane they are in.
fn tutor_app() -> (App, crate::wm::PaneId) {
    let mut app = test_app_opening(
        Config::default(),
        Box::new(MemoryStore::new()),
        Arc::new(shannon_core::MemoryVault::default()),
        crate::app::Opens::Tutor,
    );
    app.open_tutor();
    assert_eq!(
        app.focus(),
        PaneKind::Page(PageKind::Tutor),
        "premise: you are in the tutor"
    );
    let pane = app.workspace.focused();
    (app, pane)
}

/// Line `n` of the tutor as `pane` shows it.
fn line(app: &App, pane: crate::wm::PaneId, n: usize) -> String {
    let text = app.pane_page(pane).expect("a tutor window").text().text();
    text.split('\n').nth(n).expect("the line").to_string()
}

/// **The tutor is `b1`, and there is no session until one is called**
/// (Steven, 11-09: "Show the tutor in `:ls` and I don't think we should open
/// a chat until one is actually called"; 11-11, of the session the launch
/// made and `:ls` hid: "That is a secret buffer"). The prompt is aimed at the
/// lessons — no agent — and `:ls` and `:ls!` alike list the tutor alone.
/// `:new` makes the first session, here, the tutor set aside, listed, and
/// nothing said about "already on a fresh session".
#[test]
fn the_tutor_is_b1_and_no_session_exists_until_one_is_called() {
    let (mut app, pane) = tutor_app();
    let tutor = app.pane_page(pane).map(crate::page::PageWindow::buffer);
    assert_eq!(tutor.map(BufferId::number), Some(1), "the tutor is b1");
    assert_eq!(app.buffers.sessions().count(), 0, "no session made");
    assert_eq!(Some(app.prompt_target()), tutor, "aimed at the lessons");
    assert!(app.aimed_session().is_none(), "no agent");
    let rows = |app: &App, all: bool| -> Vec<String> {
        (app.buffers_listing(all).lines())
            .filter(|l| l.trim_start().starts_with('b'))
            .map(str::to_string)
            .collect()
    };
    for all in [false, true] {
        let listed = rows(&app, all);
        assert_eq!(listed.len(), 1, "the tutor alone (all: {all}): {listed:?}");
        assert!(
            listed[0].trim_start().starts_with("b1 ") && listed[0].contains("the tutor"),
            "{listed:?}"
        );
    }
    assert!(
        app.buffers_listing(false)
            .starts_with("Sessions open this run, and the tutor."),
        "the heading names it"
    );

    // Steven's keys: `:ls` over the tutor, `:b1` back to it.
    app.dispatch_command_for_test("ls");
    app.dispatch_command_for_test("b1");
    assert_eq!(
        app.pane_page(pane).map(crate::page::PageWindow::buffer),
        tutor,
        "`:b1` is the tutor"
    );

    // Called: `:new` makes the first session, shown here, and it is listed.
    app.notices.clear();
    app.dispatch_command_for_test("new");
    assert_eq!(app.buffers.sessions().count(), 1, "the first session");
    let chat = app.pane_session(pane).id;
    assert_eq!(app.prompt_target(), chat, "the prompt aims there now");
    assert!(
        !(app.notices.iter()).any(|(_, t)| t.contains("already on a fresh session")),
        "{:?}",
        app.notices
    );
    let listed = rows(&app, false);
    assert_eq!(listed.len(), 2, "the tutor and the session: {listed:?}");
}

/// **`b1` is the tutor's, whatever page is made first** — the manual and
/// the notice history made before the lessons take the next numbers, never
/// the one held, and the list stays in number order for `Buffers::get`'s
/// binary search (`Buffers::add_as`'s kind; stop 17, V2b L4: held by
/// nothing, since launch makes no page first today).
#[test]
fn the_number_held_is_the_tutors_alone() {
    let mut app = test_app_opening(
        Config::default(),
        Box::new(MemoryStore::new()),
        Arc::new(shannon_core::MemoryVault::default()),
        crate::app::Opens::Tutor,
    );
    app.dispatch_command_for_test("messages");
    app.dispatch_command_for_test("help");
    app.open_tutor();
    let tutor = app
        .pane_page(app.workspace.focused())
        .map(crate::page::PageWindow::buffer);
    assert_eq!(tutor.map(BufferId::number), Some(1), "the tutor is b1");
    let ids: Vec<BufferId> = (app.buffers.iter())
        .map(crate::app::buffers::Buffer::id)
        .collect();
    assert!(
        ids.windows(2).all(|w| w[0] < w[1]),
        "in number order: {ids:?}"
    );
    assert!(
        ids.len() >= 3,
        "premise: two pages and the tutor (no session): {ids:?}"
    );
    for id in ids {
        assert!(app.buffers.get(id).is_some(), "b{} found", id.number());
    }
}

/// **`:ball`, `:tab ball` and `:Send ball` act on what `:ls` lists**
/// (`listed_sessions`, one rule) — in the tutor, before a session is called,
/// on none, and each says so, making none: no window, no tab, no turn (stop
/// 17, V2b M3, when a session `:ls` hid got all three).
#[test]
fn ball_acts_on_what_ls_lists() {
    let (mut app, _) = tutor_app();
    let panes = app.workspace.pane_count();
    for (verb, made) in [("ball", "a window"), ("tab ball", "a tab")] {
        app.notices.clear();
        app.dispatch_command_for_test(verb);
        assert_eq!(app.workspace.pane_count(), panes, "`:{verb}` made {made}");
        assert_eq!(app.tabs.count(), 1, "`:{verb}` made {made}");
        assert!(
            app.notice()
                .is_some_and(|n| n.contains("`:ls` lists no session yet")),
            "`:{verb}` says why it did nothing: {:?}",
            app.notice()
        );
    }
    app.prompt_mut().insert_pasted("hello");
    app.dispatch_command_for_test("Send ball");
    assert_eq!(
        app.buffers.sessions().count(),
        0,
        "`:Send ball` made none: {:?}",
        app.notice()
    );
    assert_eq!(app.prompt().text(), "hello", "the draft kept");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("`:ls` lists no session yet")),
        "and says why: {:?}",
        app.notice()
    );
}

/// **A session command with no session says "no agent"** — `:rename` from
/// the tutor, before one is called, names nothing, makes nothing, and says
/// why, as `:models` does (Steven, 11-11: "refuse"). It panicked: the aim's
/// session was `expect`ed.
#[test]
fn a_session_command_with_no_session_says_no_agent() {
    for line in [
        "rename practice-chat",
        "models",
        "w",
        "continue",
        "Edit",
        "delete",
        "vsplit transcript",
        "tool ls",
        "model sonnet",
        "effort low",
        "effort",
        "skill review",
    ] {
        let (mut app, _) = tutor_app();
        app.notices.clear();
        app.dispatch_command_for_test(line);
        assert_eq!(app.buffers.sessions().count(), 0, "`:{line}` made one");
        assert!(
            (app.notices.iter()).any(|(_, t)| t.contains("no agent")),
            "`:{line}`: {:?}",
            app.notices
        );
    }
}

/// **The tutor's row says the mode, and "no agent"** — the lessons teach you
/// to watch `[NORMAL]` and `[INSERT]` on it from the first page, before any
/// session is called; it drew the red "no agent" alone (review of 850a72d,
/// HIGH). Once `:new` makes one, "no agent" goes.
#[test]
fn the_row_says_the_mode_and_no_agent_before_a_session() {
    let (mut app, _) = tutor_app();
    let row = |app: &mut App| -> String {
        let text = rendered_text(&frame_of(app, 80, 24));
        (text.lines())
            .rfind(|l| !l.trim().is_empty())
            .unwrap_or_default()
            .to_string()
    };
    let at_launch = row(&mut app);
    assert!(
        at_launch.contains("no agent") && at_launch.contains("NORMAL"),
        "{at_launch:?}"
    );
    type_keys(&mut app, "i");
    let inserting = row(&mut app);
    assert!(inserting.contains("INSERT"), "{inserting:?}");
    type_keys(&mut app, "<Esc>");
    app.dispatch_command_for_test("new");
    let with_one = row(&mut app);
    assert!(!with_one.contains("no agent"), "{with_one:?}");
}

/// **A prompt with no session is not sent, and stays** (Steven, 11-11: "it
/// can just not accept prompts") — refused with why, the draft where you
/// wrote it, no session made.
#[test]
fn the_tutor_sends_nothing_and_keeps_the_draft() {
    let (mut app, _) = tutor_app();
    app.prompt_mut().insert_pasted("hello");
    app.submit_prompt();
    assert_eq!(app.prompt().text(), "hello", "the draft kept");
    assert_eq!(app.buffers.sessions().count(), 0, "nothing made");
    assert!(
        app.notice().is_some_and(|n| n.contains("no agent")),
        "{:?}",
        app.notice()
    );
}

/// **`:vnew` from the tutor opens the first session beside it** — made
/// first, the new window born on it, as there is none to be born on; the
/// lessons stay where they were, the prompt aims at it, and it is listed.
/// `:snew` the same. (11-10 it reused a session the launch had made and
/// hidden — "That is a secret buffer", Steven, 11-11.)
#[test]
fn vnew_in_the_tutor_opens_the_first_session() {
    for cmd in ["vnew", "snew"] {
        let (mut app, tutor) = tutor_app();
        assert_eq!(app.buffers.sessions().count(), 0, "premise: none");
        app.notices.clear();

        app.dispatch_command_for_test(cmd);

        let new = app.workspace.focused();
        assert_ne!(new, tutor, "`:{cmd}`: a new pane, {:?}", app.notice());
        assert_eq!(app.buffers.sessions().count(), 1, "`:{cmd}`: one made");
        let chat = app.pane_session(new).id;
        assert_eq!(app.prompt_target(), chat, "`:{cmd}`: aimed there");
        assert!(
            app.pane_page(tutor).is_some(),
            "`:{cmd}`: the lessons stay in their window"
        );
        assert!(
            (app.buffers_listing(false).lines())
                .any(|l| l.trim_start().starts_with(&format!("b{} ", chat.number()))),
            "`:{cmd}`: listed"
        );
        assert!(
            !(app.notices.iter()).any(|(_, t)| t.contains("already on")),
            "`:{cmd}`: {:?}",
            app.notices
        );
    }
}

/// **No command takes the tutor down before a session is called** — every
/// command in the table (`crate::command::TABLE`, so a new one is in it),
/// bare and with `!` where it takes one, each in a fresh tutor, then a frame
/// drawn. The state with no session was reachable only as a broken
/// invariant, and every reader of the aim `expect`ed it (11-11).
#[test]
fn no_command_panics_in_the_tutor_before_a_session() {
    for spec in crate::command::TABLE.iter() {
        let lines = std::iter::once(spec.name.to_string())
            .chain(spec.bang.then(|| format!("{}!", spec.name)));
        for line in lines {
            let (mut app, _) = tutor_app();
            app.dispatch_command_for_test(&line);
            let _ = frame_of(&mut app, 80, 24);
        }
    }
}

/// **The tutor's window is its own**: nothing beneath it, so its `:q` is
/// the last window's and leaves, as vimtutor's does (`App::last_window`);
/// a page over it still gives it back.
#[test]
fn the_tutors_q_is_the_last_windows() {
    let (mut app, pane) = tutor_app();
    assert!(
        app.window_of(pane).is_some_and(|w| !w.has_beneath()),
        "premise: over nothing"
    );
    assert!(app.quit_window(false), "`:q` on the tutor leaves");

    // `:wq` and `:x` too, with nothing to write and nothing thrown away —
    // they stayed, told to open a session (review of 850a72d, M2) — while a
    // plain `:w` says why it wrote nothing.
    for line in ["wq", "x"] {
        let (mut app, _) = tutor_app();
        assert!(app.dispatch_command_for_test(line), "`:{line}` leaves");
    }
    let (mut app, _) = tutor_app();
    assert!(!app.dispatch_command_for_test("w"), "`:w` stays");
    assert!(
        app.notice().is_some_and(|n| n.contains("no agent")),
        "{:?}",
        app.notice()
    );
}

/// **What Lesson 2.4 says of `:ls`**: from the tutor it takes the window you
/// are reading, and `:q` gives the page back (review stop 17, F M1).
#[test]
fn ls_over_the_tutor_and_q_gives_the_page_back() {
    let (mut app, pane) = tutor_app();
    app.dispatch_command_for_test("ls");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Buffers)),
        "in the tutor's window"
    );
    app.dispatch_command_for_test("q");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Tutor)),
        "{:?}",
        app.notice()
    );
}

/// **`shannon tutor` opens with you in it** — the cursor on its first line,
/// as vimtutor starts, so `h j k l` move from the first key; the prompt
/// still there to write in.
#[test]
fn the_tutor_opens_with_the_cursor_in_it() {
    let (app, pane) = tutor_app();
    let page = app.pane_page(pane).unwrap();
    assert_eq!(page.text().position(), (0, 0));
    assert!(page.text().text().contains("Welcome to Shannon Tutor"));
    assert!(app.workspace.prompt_pane().is_some(), "the prompt stays");
}

/// **The tutor is yours to change, as vimtutor's copy is** — `x`, an insert,
/// and `u` back again, through the page's one editor; Normal after `<Esc>`.
#[test]
fn the_tutor_is_edited_in_place() {
    let (mut app, pane) = tutor_app();
    let first = line(&app, pane, 0);
    type_keys(&mut app, "x");
    assert_eq!(line(&app, pane, 0), first[1..]);
    // What is drawn is what was edited: the page's lines, painted anew.
    let painted = app.pane_page(pane).unwrap().text().lines()[0].text();
    assert_eq!(painted, first[1..], "drawn as edited");
    type_keys(&mut app, "ihello<Esc>");
    assert!(
        line(&app, pane, 0).starts_with("hello"),
        "{:?}",
        line(&app, pane, 0)
    );
    assert_eq!(app.prompt_keymap.mode(), Mode::Normal);
    type_keys(&mut app, "uu");
    assert_eq!(line(&app, pane, 0), first, "undone, twice");
}

/// **An append at a line's end types in order** — Lesson 1.3's own
/// exercise. The editing window takes the version it made
/// (`PageBuffer::edit`'s `synced`); without that, the next key re-synced it
/// through `set_lines`, whose clamp pulled the Insert cursor back off the
/// line's end, and `Ais line` typed `s linei` (review stop 16, A H1).
#[test]
fn an_append_at_a_lines_end_types_in_order() {
    let (mut app, pane) = tutor_app();
    let first = line(&app, pane, 0);
    type_keys(&mut app, "Axyz<Esc>");
    assert_eq!(line(&app, pane, 0), format!("{first}xyz"));
}

/// **A line opened in the tutor draws** — a restyle drops the page's
/// geometry (`TextView::restyle`); kept, a frame after `Go` read one line
/// past its end and panicked (review stop 16, A H1).
#[test]
fn a_line_opened_in_the_tutor_draws() {
    let (mut app, pane) = tutor_app();
    let lines = app.pane_page(pane).unwrap().text().lines().len();
    type_keys(&mut app, "Goa new line<Esc>");
    let frame = frame_of(&mut app, 80, 24);
    assert_eq!(app.pane_page(pane).unwrap().text().lines().len(), lines + 1);
    assert!(rendered_text(&frame).contains("a new line"), "drawn");
}

/// **`H` `M` `L` move by the tutor's window**, as on any page — not through
/// its editor, which has no window to find the screen in (review stop 16,
/// C 4: the arm was held by nothing).
#[test]
fn h_m_l_move_by_the_tutors_window() {
    let (mut app, pane) = tutor_app();
    let _ = frame_of(&mut app, 80, 24);
    type_keys(&mut app, "L");
    assert!(
        app.pane_page(pane).unwrap().text().position().0 > 0,
        "L went down the window: {:?}",
        app.pane_page(pane).unwrap().text().position()
    );
}

/// **An operator over `H` `M` `L` is refused in the tutor too**, as in the
/// prompt — it deleted the cursor's one line, silently (review stop 16, A
/// M2).
#[test]
fn an_operator_over_the_screen_is_refused_in_the_tutor() {
    let (mut app, pane) = tutor_app();
    let _ = frame_of(&mut app, 80, 24);
    let lines = app.pane_page(pane).unwrap().text().lines().len();
    type_keys(&mut app, "dL");
    assert_eq!(app.pane_page(pane).unwrap().text().lines().len(), lines);
    assert!(
        app.notice().is_some_and(|n| n.contains("`H` `M` `L`")),
        "{:?}",
        app.notice()
    );
}

/// **`<C-x><C-e>` in the tutor says why it does nothing** — it opens a
/// prompt's draft in `$EDITOR`, and the tutor drives the prompt's keymap, so
/// the chord arrived and was dropped, silently (review stop 16, A L3).
#[test]
fn the_editor_chord_in_the_tutor_says_why() {
    let (mut app, pane) = tutor_app();
    let before = app.pane_page(pane).unwrap().text().text();
    type_keys(&mut app, "<C-x><C-e>");
    assert!(
        app.notice().is_some_and(|n| n.contains("$EDITOR")),
        "{:?}",
        app.notice()
    );
    assert_eq!(
        app.pane_page(pane).unwrap().text().text(),
        before,
        "nothing edited"
    );
}

/// **A change with nothing to change leaves Insert unentered** — `ci(` on a
/// line with no parentheses, then `X`: in Normal, `X` deletes; left in
/// Insert, it typed into the lesson (review stop 16, C 4:
/// `operator_applied` after the page's operator was held by nothing).
#[test]
fn a_change_that_finds_nothing_leaves_insert_unentered() {
    let (mut app, pane) = tutor_app();
    let first = line(&app, pane, 0);
    type_keys(&mut app, "lci(X");
    assert_eq!(app.prompt_keymap.mode(), Mode::Normal);
    assert_eq!(
        line(&app, pane, 0),
        first[1..],
        "`X` deleted, typed nothing"
    );
}

/// **One text, every window's; one undo, the buffer's** — an edit in one
/// window on the tutor is in the other at its next sync, and `u` there
/// undoes it: vim's undo is a buffer's, its cursor a window's.
#[test]
fn an_edit_in_one_tutor_window_is_every_windows() {
    let (mut app, first_window) = tutor_app();
    app.dispatch_command_for_test("vsplit");
    let second_window = app.workspace.focused();
    assert_ne!(
        second_window, first_window,
        "premise: a second window, focused"
    );
    assert_eq!(
        app.focus(),
        PaneKind::Page(PageKind::Tutor),
        "premise: on the tutor too"
    );
    let first = line(&app, second_window, 0);
    type_keys(&mut app, "x");
    app.sync_pages();
    assert_eq!(
        line(&app, first_window, 0),
        first[1..],
        "the other window has it"
    );
    // A second edit no frame has synced: the other window is one behind,
    // and an edit there is made on what the page holds, not on its copy.
    type_keys(&mut app, "x");
    app.focus_pane(first_window);
    type_keys(&mut app, "u");
    app.sync_pages();
    assert_eq!(
        line(&app, second_window, 0),
        first[1..],
        "one undone, from the other window"
    );
    type_keys(&mut app, "u");
    app.sync_pages();
    assert_eq!(line(&app, second_window, 0), first, "and the other");
}

/// **The other window follows its text through an edit** — lines deleted
/// above its cursor move it with the text it was on, as vim's other windows
/// follow a buffer's change. It kept its index, and stood on another line
/// (review stop 16, A M1, measured).
#[test]
fn another_tutor_window_follows_its_text_through_an_edit() {
    let (mut app, first_window) = tutor_app();
    app.dispatch_command_for_test("vsplit");
    let second_window = app.workspace.focused();
    app.focus_pane(first_window);
    type_keys(&mut app, "40G");
    let at = app.pane_page(first_window).unwrap().text().position().0;
    let reading = line(&app, first_window, at);
    app.focus_pane(second_window);
    type_keys(&mut app, "gg5dd");
    app.sync_pages();
    let now = app.pane_page(first_window).unwrap().text().position().0;
    assert_eq!(now, at - 5, "moved up with its text");
    assert_eq!(line(&app, first_window, now), reading, "on the same text");
}

/// **A picture is as tall as the pane would be drawn** (`approval_max_height`
/// and its two borders), and a marker with no command above it draws nothing
/// (review stop 17, C 11).
#[test]
fn a_picture_is_capped_and_a_lone_marker_draws_nothing() {
    let picture = |cap: u16| {
        let mut config = Config::default();
        config.tui.approval_max_height = cap;
        let app = test_app_with(config);
        let command = "mkdir -p a && echo hi | tee a/x | sort | uniq | wc -l && rm a/x";
        let text = format!("```tool bash\n{command}\n```\n{}\n", crate::tutor::DECISION);
        let filled = crate::tutor::drawn_in(&text, &app.tutor_pictures(&text));
        filled
            .split('\n')
            .filter(|r| r.starts_with(['┌', '│', '└']))
            .count()
    };
    assert_eq!(picture(3), 5, "three rows and the borders");
    assert!(picture(18) > 5, "premise: uncapped it is taller");
    let app = test_app();
    let lone = crate::tutor::DECISION;
    let pictures = app.tutor_pictures(lone);
    assert_eq!(
        crate::tutor::drawn_in(lone, &pictures),
        lone,
        "drawn as nothing"
    );
}

/// **A theme changed between an edit and the other window's sync loses
/// neither** — the repaint is a version too, and no line moved in it (review
/// stop 17, A L1).
#[test]
fn a_new_theme_keeps_another_window_on_its_text() {
    let (mut app, first_window) = tutor_app();
    app.dispatch_command_for_test("vsplit");
    let second_window = app.workspace.focused();
    app.focus_pane(first_window);
    type_keys(&mut app, "40G");
    let at = app.pane_page(first_window).unwrap().text().position().0;
    let reading = line(&app, first_window, at);
    app.focus_pane(second_window);
    type_keys(&mut app, "gg5dd");
    app.set_theme(crate::theme::Theme::terminal());
    app.sync_pages();
    let now = app.pane_page(first_window).unwrap().text().position().0;
    assert_eq!(now, at - 5, "moved up with its text");
    assert_eq!(line(&app, first_window, now), reading, "on the same text");
}

/// **A theme changed with an Insert open leaves its cursor past the line's
/// end** — a repaint moves nothing, and clamping one put what you typed next
/// a character early (review stop 17, A L2).
#[test]
fn a_new_theme_leaves_an_inserts_cursor_alone() {
    let (mut app, pane) = tutor_app();
    let first = line(&app, pane, 0);
    type_keys(&mut app, "A");
    app.set_focus(PaneKind::Prompt);
    app.set_theme(crate::theme::Theme::terminal());
    app.sync_pages();
    app.focus_pane(pane);
    type_keys(&mut app, "xy");
    assert_eq!(line(&app, pane, 0), format!("{first}xy"));
}

/// **`<CR>` in the tutor sends nothing** — it takes the prompt's keymap,
/// whose Normal `<CR>` sends; from there it sent the PROMPT's draft, unseen.
/// Refused, and said.
#[tokio::test]
async fn the_tutor_is_never_sent() {
    let (mut app, _) = tutor_app();
    app.prompt_mut().insert_pasted("a draft in the prompt");
    type_keys(&mut app, "<CR>");
    assert!(!app.streaming(), "no turn");
    assert_eq!(app.prompt().text(), "a draft in the prompt");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("the tutor is never sent")),
        "{:?}",
        app.notice()
    );
}

/// **A yank in the tutor is a `p` in the prompt** — the page's editor shares
/// the registers every draft does. And `yy` as the FIRST keys after the
/// tutor opens: it opens at launch, in no event, and the first key's end
/// took the focus it moved for one of its own and dropped the `y` it began
/// (`App::open_tutor`; `tutor_app` settles nothing, as launch does not).
#[test]
fn a_yank_in_the_tutor_is_a_put_in_the_prompt() {
    let (mut app, pane) = tutor_app();
    let first = line(&app, pane, 0);
    type_keys(&mut app, "yy");
    assert_eq!(
        app.prompt().register_copied().map(|c| c.text()),
        Some(format!("{first}\n")),
        "the yank filled the one register"
    );
    app.set_focus(PaneKind::Prompt);
    type_keys(&mut app, "P");
    assert_eq!(app.prompt().text().lines().next(), Some(first.as_str()));
}

/// **A yank in the tutor reaches the system clipboard** — the lesson's own
/// keys, `yy` and then `V2jy` (Lesson 2.3): the page's editor fills the
/// register, and the clipboard is sent what it took, as a prompt's yank is.
/// The register was tested; the clipboard was not (Steven, 11-08: "I was
/// struggling to figure out how to copy to my clipboard").
#[test]
fn a_yank_in_the_tutor_reaches_the_clipboard() {
    let (mut app, pane) = tutor_app();
    let clip = MemoryClipboard::default();
    app.osc = Box::new(clip.clone());
    let first = line(&app, pane, 0);
    type_keys(&mut app, "yy");
    assert_eq!(clip.last(), Some(format!("{first}\n")), "yy");
    let three: Vec<String> = (0..3).map(|n| line(&app, pane, n)).collect();
    type_keys(&mut app, "V2jy");
    assert_eq!(clip.last(), Some(format!("{}\n", three.join("\n"))), "V2jy");
}

/// **The tutor's picture of a decision is the pane, drawn** (Steven,
/// 11-08: "We should be rendering just as shannon's would… Make sure it
/// paints accurately!") — for the lesson's command, under this session's
/// policy, by the pane's own draw: its title, its number, its verdict, the
/// command laid out, its legend with its keys in the colour the pane gives
/// them; at the lessons' width.
#[test]
fn the_tutors_picture_of_a_decision_is_the_pane_drawn() {
    let app = test_app();
    let command = "mkdir -p permtest && echo hi | tee permtest/x";
    let text = format!("```tool bash\n{command}\n```\n{}\n", crate::tutor::DECISION);
    let pictures = app.tutor_pictures(&text);
    let filled = crate::tutor::drawn_in(&text, &pictures);
    let page = crate::tutor::lines(&filled, &pictures);
    let rows: Vec<&str> = filled
        .split('\n')
        .skip(3)
        .take_while(|r| !r.is_empty())
        .collect();
    assert!(rows[0].starts_with("┌ run this? · <id> "), "{}", rows[0]);
    assert!(rows[0].ends_with("2─┐"), "numbered: {}", rows[0]);
    assert!(rows.last().is_some_and(|r| r.starts_with('└')), "{rows:?}");
    let width = |r: &str| crate::wrap::str_cells(r);
    assert!(
        rows.iter()
            .all(|r| width(r) == crate::tutor::WIDTH as usize),
        "{rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("ASK")),
        "its verdict: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("mkdir -p permtest")),
        "its command: {rows:?}"
    );
    // The legend, painted as the pane paints it: its keys in what you
    // type's colour.
    let legend = (3..3 + rows.len())
        .find(|&n| page[n].text().contains("answer:"))
        .expect("a legend row");
    let theme = app.theme();
    let code = crate::view::painter(PaneKind::Tool, theme)
        .style(&crate::paint::Run::role(
            "x",
            crate::style::CellStyle::ink(crate::style::Ink::HelpCode),
        ))
        .fg;
    let key = (page[legend].runs.iter())
        .find(|run| run.text.contains("d/q"))
        .expect("the deny key");
    assert!(
        matches!(key.look, crate::paint::Look::Own(style) if style.fg == code),
        "{key:?}"
    );
}

/// **The tutor opens with every picture its lessons mark drawn in** — the
/// page `shannon tutor` opens, through `open_tutor`, not a text made for
/// the test: a marker left as written would be `{DECISION}` on the page.
#[test]
fn the_tutor_opens_with_its_pictures_drawn() {
    let (app, pane) = tutor_app();
    let text = app.pane_page(pane).unwrap().text().text();
    let marked = crate::tutor::Pictures::commands(&crate::tutor::text()).len();
    assert!(!text.contains(crate::tutor::DECISION), "every marker drawn");
    assert_eq!(
        text.matches("┌ run this? · <id> ").count(),
        marked,
        "one picture each"
    );
}

/// **The tutor's pictures wear a new theme** — they are cells, drawn once,
/// and the lesson says to try `:themes` right below one. Its rendered key,
/// in each theme in turn.
#[test]
fn the_tutors_pictures_wear_a_new_theme() {
    use ratatui::style::Color;
    let dir = tempfile::tempdir().unwrap();
    for (name, code) in [("one", "#ff0000"), ("two", "#00ff00")] {
        let file = format!("\"ui.help.code\" = \"{code}\"\n");
        std::fs::write(dir.path().join(format!("{name}.toml")), file).unwrap();
    }
    let theme = |name| {
        (crate::theme::Theme::load(name, Some(dir.path())))
            .expect("loads")
            .0
    };
    let (mut app, pane) = tutor_with_a_picture();
    app.set_theme(theme("one"));
    assert_eq!(picture_key(&mut app, pane), Some(Color::Rgb(0xff, 0, 0)));
    app.set_theme(theme("two"));
    assert_eq!(picture_key(&mut app, pane), Some(Color::Rgb(0, 0xff, 0)));
}

/// **An edit leaves a picture as it was drawn** — the page is painted anew
/// on each edit, its pictures with it (review stop 17, C 3).
#[test]
fn a_picture_keeps_its_colours_through_an_edit() {
    let (mut app, pane) = tutor_with_a_picture();
    let drawn = picture_key(&mut app, pane);
    assert!(drawn.is_some(), "premise: the legend's key has its colour");
    type_keys(&mut app, "x");
    assert_eq!(line(&app, pane, 0), "irst line", "premise: edited");
    assert_eq!(picture_key(&mut app, pane), drawn);
}

/// A tutor of its own text, holding one picture: what the pictures do, read
/// whatever the lessons hold just now.
fn tutor_with_a_picture() -> (App, crate::wm::PaneId) {
    let mut app = test_app();
    let text = format!(
        "first line\n```tool bash\nmkdir -p a && echo hi | tee a/x\n```\n{}\n",
        crate::tutor::DECISION
    );
    let lessons = app.tutor_lessons(text);
    app.page_for(crate::page::Recipe::Tutor(lessons));
    app.open_tutor();
    let pane = app.workspace.focused();
    (app, pane)
}

/// The colour of the deny key in the one picture `pane` shows.
fn picture_key(app: &mut App, pane: crate::wm::PaneId) -> Option<ratatui::style::Color> {
    frame_of(app, 100, 40);
    let lines = app.pane_page(pane).unwrap().text().lines();
    let legend = (lines.iter())
        .find(|row| row.text().starts_with('│') && row.text().contains("answer:"))
        .expect("the picture's legend");
    let key = (legend.runs.iter())
        .find(|run| run.text.contains("d/q"))
        .expect("the deny key");
    match key.look {
        crate::paint::Look::Own(style) => style.fg,
        _ => None,
    }
}

/// **A paste lands in the tutor**, as one undo — and not in the prompt.
#[test]
fn a_paste_lands_in_the_tutor() {
    let (mut app, pane) = tutor_app();
    app.paste("pasted ");
    assert!(
        line(&app, pane, 0).starts_with("pasted "),
        "{:?}",
        line(&app, pane, 0)
    );
    assert!(app.prompt().text().is_empty(), "not the prompt's");
    type_keys(&mut app, "u");
    assert!(!line(&app, pane, 0).contains("pasted"));
}

/// **Normal mode's cursor rests on a char** — `$x` deletes a line's last,
/// and the cursor steps back onto the new last, as the prompt's does
/// (`enforce_normal_cursor`).
#[test]
fn the_tutors_normal_cursor_rests_on_a_char() {
    let (mut app, pane) = tutor_app();
    type_keys(&mut app, "$x");
    let len = line(&app, pane, 0).chars().count();
    assert_eq!(app.pane_page(pane).unwrap().text().position(), (0, len - 1));
}

/// **An insert begun in the tutor and left is not replayed** — its editor is
/// its buffer's, so leaving its window drops the count, as a prompt's draft
/// drops it (`an_insert_begun_in_a_prompt_left_is_not_replayed`).
#[test]
fn an_insert_begun_in_the_tutor_and_left_is_not_replayed() {
    let (mut app, pane) = tutor_app();
    type_keys(&mut app, "3iab");
    let prompt = app.workspace.prompt_pane().expect("a prompt");
    app.focus_pane(prompt);
    type_keys(&mut app, "xy<Esc>");
    assert_eq!(app.prompt().text(), "xy", "premise: typed there, once");
    app.focus_pane(pane);
    type_keys(&mut app, "iz<Esc>");
    let first = line(&app, pane, 0);
    assert_eq!(
        (first.matches('a').count(), first.matches('z').count()),
        (1, 1),
        "nothing replayed: {first:?}"
    );
}

/// **`:b N` back to the tutor, then a key before any frame, edits where you
/// were** — the cursor goes to its line at once; only the line's place in
/// the window waits for the frame. Held whole, `x` the loop drained before
/// drawing cut line 1's first char (review stop 16, A L6, measured). `:close`,
/// not `:new`: a window set aside comes back whole, and `:b N` makes a new
/// one, through `reveal`, only for a page with none
/// (`PageBuffer::returned_to`). (It was `:q`, which gave the transcript
/// back; the tutor's window holds none now, and its `:q` leaves.)
#[test]
fn a_key_before_the_frame_after_b_edits_where_you_were() {
    let (mut app, pane) = tutor_app();
    let tutor = app.pane_page(pane).unwrap().buffer().number();
    // A line far down that is not the first — found, not counted: the
    // lessons are Steven's to rewrite, and a fixed `50G` read a blank line
    // the day he did.
    let first = line(&app, pane, 0);
    let far = (40..)
        .find(|&n| {
            let text = line(&app, pane, n);
            !text.is_empty() && text != first
        })
        .expect("a line far down");
    type_keys(&mut app, &format!("{}G", far + 1));
    let fiftieth = line(&app, pane, far);
    assert!(
        !fiftieth.is_empty() && fiftieth != first,
        "premise: {fiftieth:?}"
    );
    app.dispatch_command_for_test("close");
    assert_ne!(
        app.focus(),
        PaneKind::Page(PageKind::Tutor),
        "premise: left"
    );
    app.dispatch_command_for_test(&format!("b {tutor}"));
    assert_eq!(
        app.focus(),
        PaneKind::Page(PageKind::Tutor),
        "premise: back"
    );
    let pane = app.workspace.focused();
    type_keys(&mut app, "x");
    assert_eq!(line(&app, pane, 0), first, "line 1 untouched");
    assert_eq!(line(&app, pane, far), fiftieth[1..], "where you were");
}

/// **A click in an insert ends its count and lands where you pointed** —
/// `3iq`, a click on another line, `z<Esc>`: one `q`, one `z`, as an arrow
/// in vim ends the count (its `arrow_used`). And Insert's cursor rests past a
/// line's last char, so a click past the end lands there; Normal's lands on
/// the last char (review stop 16, A M7 and L5).
#[test]
fn a_click_in_an_insert_ends_its_count_and_lands_past_the_end() {
    let (mut app, pane) = tutor_app();
    let area = Rect::new(0, 0, 200, 50);
    app.before_frame(area.width, area.height);
    let rect = app
        .workspace
        .frame_layout(area)
        .pane(pane)
        .expect("on screen")
        .rect;
    assert!(rect.width >= 90, "premise: the first lines draw unwrapped");
    // The movement diagram's `j` — a short line on the first screen, found
    // by what it says rather than where it was (the lessons move).
    let row = (0..20)
        .find(|&n| line(&app, pane, n).trim().trim_matches('`') == "j")
        .expect("premise: the diagram's short `j` line");
    let short = line(&app, pane, row);
    let past = Position::new(rect.x + 60, rect.y + 1 + row as u16);
    let qz = |app: &App| {
        let text = app.pane_page(pane).unwrap().text().text();
        (text.matches('q').count(), text.matches('z').count())
    };
    let (q, z) = qz(&app);
    type_keys(&mut app, "3iq");
    app.place_cursor_at_mouse_for_test(area, past);
    assert_eq!(
        app.pane_page(pane).unwrap().text().position(),
        (row, short.len())
    );
    type_keys(&mut app, "z<Esc>");
    assert_eq!(
        qz(&app),
        (q + 1, z + 1),
        "nothing replayed: {:?} / {:?}",
        line(&app, pane, 0),
        line(&app, pane, row)
    );
    app.place_cursor_at_mouse_for_test(area, past);
    let last = line(&app, pane, row).len() - 1;
    assert_eq!(
        app.pane_page(pane).unwrap().text().position(),
        (row, last),
        "Normal clamps"
    );
}

/// **A tutor launch approves no unasked write outside its own directory** —
/// the knowledge roots and the scratch it chose (`tutor::Launch`), handed to
/// the app as `main` hands them. With your config directory and home as
/// roots, as an ordinary launch reads them, the same launch made
/// `projects/<its directory>/memory` there and approved writes into it
/// (review stop 16, D 1, measured) — asked here too, so the probe is known
/// to see one.
#[test]
fn a_tutor_launch_approves_no_write_outside_its_directory() {
    let (parent, yours) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let launch = crate::tutor::Launch::prepare(parent.path()).expect("launches");
    let dir = launch.dir.clone();
    let real = dir.canonicalize().unwrap();
    let launched = |knowledge: (Option<std::path::PathBuf>, Option<std::path::PathBuf>),
                    scratch: Vec<String>| {
        let mut app = test_app_with(launch.config.clone());
        app.start = Some(dir.clone());
        app.set_scratch(scratch);
        let (config_dir, home) = knowledge;
        let _ = app.launch_knowledge(config_dir, home);
        app
    };
    let outside = |app: &App| -> Vec<String> {
        (app.memory_write.iter())
            .chain(&app.claude_memory_write)
            .chain(&app.scratch)
            .filter(|p| !std::path::Path::new(p).starts_with(&dir))
            .filter(|p| !std::path::Path::new(p).starts_with(&real))
            .cloned()
            .collect()
    };
    let tutor = launched(launch.knowledge(), launch.scratch());
    assert_eq!(outside(&tutor), Vec::<String>::new());
    let ordinary = launched(
        (
            Some(yours.path().join("config")),
            Some(yours.path().join("home")),
        ),
        Vec::new(),
    );
    assert!(
        !outside(&ordinary).is_empty(),
        "premise: the probe sees a write outside"
    );
    assert!(
        yours.path().join("config/projects").is_dir(),
        "premise: and one made"
    );
}

/// **A tutor launch reads nothing above its own directory** (review stop
/// 16, E 4, measured): under a parent holding a `.git`, a planted skill
/// and an `AGENTS.md` — what anyone can make in `/tmp` — the walk climbed
/// to the parent and granted reads of what it found. The tutor's settings
/// mark no project root; the same launch with the default markers grants
/// the read, so the probe is known to see one.
#[test]
fn a_tutor_launch_reads_nothing_above_its_directory() {
    use shannon_core::permission::Decision;
    let parent = TempDir::new().unwrap();
    std::fs::create_dir(parent.path().join(".git")).unwrap();
    let skill = parent.path().join(".shannon/skills/planted/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, "---\nname: planted\ndescription: x\n---\nplanted\n").unwrap();
    std::fs::write(parent.path().join("AGENTS.md"), "planted instructions\n").unwrap();
    let launch = crate::tutor::Launch::prepare(parent.path()).expect("launches");
    let read = format!("cat '{}'", skill.display());
    let launched = |config: shannon_core::Config| {
        let mut app = test_app_with(config);
        app.start = Some(launch.dir.clone());
        let (config_dir, home) = launch.knowledge();
        let _ = app.launch_knowledge(config_dir, home);
        app.rules.decide_line(&read).decision
    };
    assert!(launch.config.memory.project_root.is_empty());
    assert_eq!(launched(launch.config.clone()), Decision::Ask);
    let mut marked = launch.config.clone();
    marked.memory.project_root = shannon_core::config::MemoryConfig::default().project_root;
    assert_eq!(
        launched(marked),
        Decision::Allow,
        "premise: with the default markers the walk reaches the parent"
    );
}

/// **`:reload` in the tutor reads no file of yours** — it says why, and the
/// stub still answers: a reload that laid your model back over the tutor's
/// would answer its lessons from a real one.
#[test]
fn reload_in_the_tutor_keeps_the_stub() {
    let mut app = test_app();
    // As launch makes it (`tutor::Launch`), not by hand (stop 16, C 1).
    let parent = TempDir::new().unwrap();
    let launch = crate::tutor::Launch::prepare(parent.path()).expect("launches");
    app.set_config_stack_without_yours(launch.stack, crate::tutor::READS_NO_CONFIG);
    let _ = app.adopt_config(launch.config);
    assert_eq!(app.config.tui.model, "stub", "premise: the tutor's model");
    app.dispatch_command_for_test("reload");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains(crate::tutor::READS_NO_CONFIG)),
        "{:?}",
        app.notice()
    );
    assert_eq!(app.config.tui.model, "stub");
    assert_eq!(app.config.tui.prompt.height, 3);
}

/// **A line `- |name|` written above the tutor's first**, as a lesson's list
/// of help files reads. The lessons are Steven's to reword, so a test that
/// needs a link writes its own: these named the lessons' `|motions|`, which
/// only his uncommitted text held, and every commit was red without it
/// (stop 17, V2b M1).
fn write_link(app: &mut App, name: &str) {
    type_keys(app, &format!("ggO- |{name}|<Esc>"));
}

/// `pane`'s cursor put on the first line holding `|name|`, at its first
/// column, as a reader arrives on a lesson's `- |motions|`; that line.
fn on_link(app: &mut App, pane: crate::wm::PaneId, name: &str) -> usize {
    let bars = format!("|{name}|");
    let window = (app.window_of_mut(pane))
        .and_then(Window::page_mut)
        .expect("a page");
    let at = (window.text().lines().iter())
        .position(|line| line.text().contains(&bars))
        .expect("a line holding the link");
    window.text_mut().goto_line(at, 0);
    at
}

/// The focused page's cursor line.
fn cursor_line(app: &App) -> usize {
    app.page_view().expect("a page").text().cursor()
}

/// **`<C-]>` in the tutor opens the manual at the link, and `<C-t>` gives
/// the tutor back where it was** (Steven, 11-09: the lessons list help
/// files, and no key followed one; review stop 17, F M5).
#[test]
fn a_link_in_the_tutor_opens_the_manual_and_ctrl_t_gives_the_tutor_back() {
    let (mut app, pane) = tutor_app();
    write_link(&mut app, "motions");
    let at = on_link(&mut app, pane, "motions");
    type_keys(&mut app, "<C-]>");
    assert_eq!(app.focus(), PaneKind::Page(PageKind::Help));
    assert_eq!(Some(cursor_line(&app)), crate::help_view::anchor("motions"));
    assert!(
        app.page_view().unwrap().title().contains("motions"),
        "{}",
        app.page_view().unwrap().title()
    );
    type_keys(&mut app, "<C-t>");
    assert_eq!(app.focus(), PaneKind::Page(PageKind::Tutor));
    assert_eq!(app.workspace.focused(), pane);
    assert_eq!(cursor_line(&app), at, "where it was followed from");
}

/// **`<C-t>` walks back through the manual, cursor and title**: to the
/// exact column, so `<C-]>` from there follows the same link; a count goes
/// that many; the stack survives the window being covered; and at the
/// oldest entry, what the manual covered comes back.
#[test]
fn ctrl_t_walks_back_through_the_manual() {
    let mut app = test_app();
    app.dispatch_command_for_test("help");
    let pane = app.workspace.focused();
    assert_eq!(app.focus(), PaneKind::Page(PageKind::Help), "premise");

    let first = on_link(&mut app, pane, "search");
    type_keys(&mut app, "<C-]>");
    assert_eq!(Some(cursor_line(&app)), crate::help_view::anchor("search"));
    let second = on_link(&mut app, pane, "motions");
    type_keys(&mut app, "3l");
    let was = app.page_view().unwrap().text().position();
    assert_eq!(was.1, 3, "premise: off the first column");
    type_keys(&mut app, "<C-]>");
    assert_eq!(Some(cursor_line(&app)), crate::help_view::anchor("motions"));

    // Covered and given back, the stack is still the window's.
    app.dispatch_command_for_test("messages");
    assert_eq!(app.focus(), PaneKind::Page(PageKind::Messages), "premise");
    app.dispatch_command_for_test("q");
    assert_eq!(app.focus(), PaneKind::Page(PageKind::Help), "premise");

    type_keys(&mut app, "<C-t>");
    assert_eq!(app.page_view().unwrap().text().position(), was);
    assert_eq!(cursor_line(&app), second);
    assert!(
        app.page_view().unwrap().title().contains("search"),
        "its title as it was: {}",
        app.page_view().unwrap().title()
    );
    type_keys(&mut app, "<C-]>");
    assert_eq!(Some(cursor_line(&app)), crate::help_view::anchor("motions"));

    type_keys(&mut app, "2<C-t>");
    assert_eq!(cursor_line(&app), first, "a count goes that many");
    assert_eq!(app.focus(), PaneKind::Page(PageKind::Help), "premise");
    type_keys(&mut app, "<C-t>");
    assert_ne!(
        app.focus(),
        PaneKind::Page(PageKind::Help),
        "what the manual covered comes back"
    );
}

/// **`<CR>` follows a link and is still refused on a line without one**,
/// and in Insert it is the newline it always was.
#[test]
fn enter_follows_a_link_only_where_there_is_one() {
    let (mut app, pane) = tutor_app();
    type_keys(&mut app, "<CR>");
    assert_eq!(app.focus(), PaneKind::Page(PageKind::Tutor));
    assert!(
        app.notice().is_some_and(|n| n.contains("never sent")),
        "{:?}",
        app.notice()
    );

    write_link(&mut app, "motions");
    on_link(&mut app, pane, "motions");
    let rows = app.pane_page(pane).unwrap().text().lines().len();
    type_keys(&mut app, "i<CR><Esc>");
    assert_eq!(app.focus(), PaneKind::Page(PageKind::Tutor), "a newline");
    assert_eq!(app.pane_page(pane).unwrap().text().lines().len(), rows + 1);
    type_keys(&mut app, "u");

    on_link(&mut app, pane, "motions");
    type_keys(&mut app, "<CR>");
    assert_eq!(app.focus(), PaneKind::Page(PageKind::Help));
    assert_eq!(Some(cursor_line(&app)), crate::help_view::anchor("motions"));
}

/// **No key is dead**: each place these keys do nothing says so.
#[test]
fn the_link_keys_say_why_when_they_do_nothing() {
    let says = |app: &App, words: &str| {
        assert!(
            app.notice().is_some_and(|n| n.contains(words)),
            "{words}: {:?}",
            app.notice()
        );
    };
    let (mut app, _) = tutor_app();
    type_keys(&mut app, "<C-]>");
    says(&app, "no |link| here");
    type_keys(&mut app, "<C-t>");
    says(&app, "comes back from a link");

    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    type_keys(&mut app, "<C-]>");
    says(&app, "in `:help` and the tutor");

    // A manual no jump opened: a split of one has nothing to go back to.
    let mut app = test_app();
    app.split_command("help", SplitDir::Horizontal);
    assert_eq!(app.focus(), PaneKind::Page(PageKind::Help), "premise");
    type_keys(&mut app, "<C-t>");
    says(&app, "bottom of the tag stack");
    assert_eq!(app.focus(), PaneKind::Page(PageKind::Help), "it stays");
}

/// **A link to nothing is refused as `:help` of it is**, one door.
#[test]
fn a_link_to_nothing_says_what_help_says() {
    let mut app = test_app();
    let lessons = app.tutor_lessons("- |no-such-place|\n".to_string());
    app.page_for(crate::page::Recipe::Tutor(lessons));
    app.open_tutor();
    type_keys(&mut app, "<C-]>");
    assert_eq!(app.focus(), PaneKind::Page(PageKind::Tutor), "it stays");
    assert_eq!(
        app.notice(),
        Some(crate::help::no_such_topic("no-such-place"))
    );
}
