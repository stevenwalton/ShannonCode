//! `:ls`, `:buffer`, `:Send` to a session in no pane; buffer ids and prefixes.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

// --- buffers: :ls, :buffer, :Send to a session in no pane -------------------

/// **A turn is a prompt, not a tool round.** A tool's result travels as a
/// user message, and `:ls` counted user messages: one question with a tool
/// round read "2 turns" (Reviewer D, round 17).
#[test]
fn ls_counts_prompts_not_tool_rounds() {
    let mut app = test_app();
    let c = app.conversation_mut();
    c.begin_turn("list the files");
    c.push_tool_use(shannon_core::Block::ToolUse {
        id: "tu_1".into(),
        name: "bash".into(),
        input: Default::default(),
    });
    c.finish_tool_round(shannon_core::Message::tool_result("tu_1", "a b c", false));
    c.push_chunk("three files");
    c.commit_turn();
    let row = ls_row(&app, 1);
    assert!(
        row.contains("  1 turn ") && row.contains("list the files"),
        "{row}"
    );
}

/// **A session in no pane is still open, and `:ls` says so** — and `:buffer`
/// brings it back. Steven: open a chat, prompt it, replace the view with
/// another, without killing the first.
#[tokio::test]
async fn ls_lists_a_hidden_session_and_buffer_brings_it_back() {
    let mut app = test_app();
    app.submit("first session");
    assert!(app.answering(), "premise: a turn runs on b1");
    app.new_session_command();

    assert!(
        ls_row(&app, 1).contains(" h "),
        "b1 hidden: {}",
        ls_row(&app, 1)
    );
    assert!(
        ls_row(&app, 1).contains("running"),
        "and still working: {}",
        ls_row(&app, 1)
    );
    assert!(ls_row(&app, 1).contains("first session"));
    assert!(
        ls_row(&app, 2).contains("%a"),
        "b2 shown and aimed: {}",
        ls_row(&app, 2)
    );

    app.dispatch_command_for_test("buffer 1");
    assert!(ls_row(&app, 1).contains("%a"), "{}", ls_row(&app, 1));
    assert!(ls_row(&app, 2).contains(" h "), "{}", ls_row(&app, 2));
    app.dispatch_command_for_test("b 9");
    assert!(
        app.notice().unwrap_or_default().contains("no buffer"),
        "{:?}",
        app.notice()
    );
}

/// **`:Send` and `:child` claim no secret withheld for a plain alias** —
/// neither says a secret was kept back for the home's path (Steven, 10-23,
/// image 7; `Aliases::secrets_among`, read by every sentence that says what
/// was withheld — the typed prompt's is
/// `submitting_says_how_many_secrets_were_withheld`). That the path leaves
/// as `~` is the scrub's, held by `home_is_aliased_to_a_portable_tilde_by_
/// default`; this said it of both doors and asserted neither (review stop
/// 12, T4 8).
#[tokio::test]
async fn send_and_child_claim_no_secret_for_the_homes_path() {
    let home = std::env::var("HOME").expect("a home to name");
    let mut app = test_app();
    two_sessions_b1_hidden(&mut app);
    app.prompt_mut()
        .insert_pasted(&format!("look in {home}/proj"));
    app.dispatch_command_for_test("Send b1");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("sent to 1 session"), "premise: sent — {said}");
    assert!(!said.contains("withheld"), "{said}");

    // A child hangs off a turn.
    let mut app = test_app();
    app.conversation_mut().begin_turn("a turn to hang off");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    app.spawn_child_job(&format!("look in {home}/proj"), None)
        .expect("spawned");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("in the background"),
        "premise: spawned — {said}"
    );
    assert!(!said.contains("withheld"), "{said}");
}

/// `:Send b1` reaches a session no pane shows, as a job, and leaves the
/// marks alone — there is no pane to mark. The notice names who it went to.
#[tokio::test]
async fn send_reaches_a_hidden_session_by_buffer_number() {
    let mut app = test_app();
    // A committed turn, so `:new` really leaves b1 — on a blank session it
    // is reused, and b1 stayed SHOWN: this test measured a send to the shown
    // session while its comment said hidden (Reviewer C, round 16). The
    // fixture asserts it.
    two_sessions_b1_hidden(&mut app);
    app.prompt_mut().insert_pasted("to the hidden one");
    app.dispatch_command_for_test("Send b1");
    assert!(
        app.generations.iter().any(|g| g.buffer == BufferId(0)),
        "a job runs on b1: {:?}",
        app.notice()
    );
    assert!(
        app.notice()
            .unwrap_or_default()
            .contains("sent to 1 session (b1 "),
        "the notice names the recipient: {:?}",
        app.notice()
    );
    assert!(app.workspace.marked_panes().is_empty(), "no marks were set");
    assert!(app.prompt().text().is_empty(), "the prompt was consumed");

    app.prompt_mut().insert_pasted("again");
    app.dispatch_command_for_test("Send b7");
    assert!(
        app.notice().unwrap_or_default().contains("no buffer b7"),
        "{:?}",
        app.notice()
    );
    assert_eq!(
        app.prompt().text(),
        "again",
        "a refused send keeps the text"
    );
}

/// **What `:ls` exists to say**: a session no pane shows is waiting on you.
/// A job on a hidden buffer asked; its row says `needs you`, the shown one's
/// does not.
#[tokio::test]
async fn ls_says_which_hidden_session_needs_you() {
    let mut app = test_app();
    // A committed turn, so `:new` really leaves b1 (a blank one is reused).
    two_sessions_b1_hidden(&mut app);
    app.prompt_mut().insert_pasted("work out of sight");
    app.dispatch_command_for_test("Send b1");
    let job = app
        .generations
        .iter()
        .find(|g| g.buffer == BufferId(0))
        .map(|g| g.id)
        .expect("premise: a job on b1");
    app.gate_job_tool_call_for_test(job, "mkdir build");
    assert!(app.shown_call().is_some(), "premise: it asked");
    assert!(ls_row(&app, 1).contains("needs you"), "{}", ls_row(&app, 1));
    assert!(
        !ls_row(&app, 2).contains("needs you"),
        "{}",
        ls_row(&app, 2)
    );
}

/// A HELD session waits on you as surely as one with a decision open, and
/// `:ls` says so in `:graph`'s words — it said only "held" (Reviewer F, round
/// 16), and no test reached the state at all (Reviewer C).
#[tokio::test]
async fn ls_names_a_held_session_as_needing_you() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("long task", None).unwrap();
    let job = app
        .generations
        .iter()
        .find(|g| matches!(&g.work, Work::Job(j) if j.episode == child))
        .map(|g| g.id)
        .expect("premise: the child's job");
    app.gate_job_tool_call_for_test(job, "mkdir build");
    assert!(
        app.job_of(job).is_some_and(|j| j.held.is_some()),
        "premise: an unattended child holds rather than asks"
    );
    let short = &child.to_hex()[..8];
    let listing = app.buffers_listing(false);
    let row = listing
        .lines()
        .find(|l| l.contains(short))
        .unwrap_or_else(|| panic!("the child's row:\n{listing}"));
    assert!(row.contains("held — needs you"), "{row}");
    // Its briefing is its one turn: "1 turn", not "1 turns".
    assert!(
        row.contains(" 1 turn ") && !row.contains("1 turns"),
        "{row}"
    );
}

/// **`:ls` is a live pane**: a hidden session that starts to need you says so
/// on the page you already have open, with no keypress, and your place on it
/// is kept. It was a snapshot in `$EDITOR`, which froze the listing and the
/// event loop with it — nothing could arrive while you read (Reviewer F,
/// round 16; Steven: a live pane).
#[tokio::test]
async fn ls_is_a_live_pane_that_keeps_your_place() {
    let mut app = test_app();
    two_sessions_b1_hidden(&mut app);
    app.prompt_mut().insert_pasted("work out of sight");
    app.dispatch_command_for_test("Send b1");
    let job = app
        .generations
        .iter()
        .find(|g| g.buffer == BufferId(0))
        .map(|g| g.id)
        .expect("premise: a job on b1");

    app.dispatch_command_for_test("ls");
    let pane = app
        .workspace
        .first_of(PaneKind::Page(PageKind::Buffers))
        .expect("`:ls` opens a pane");
    app.clamp_text_panes(100, 30);
    let page = |app: &App| -> String {
        app.pane_listing(pane)
            .expect("content")
            .lines()
            .iter()
            .map(|l| l.text())
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(!page(&app).contains("needs you"), "premise: {}", page(&app));
    app.text_pane_mut_for_test(pane)
        .expect("a text page")
        .cursor_bottom(0);
    let place = app.text_pane_pos_for_test(pane).expect("a text page").0;

    app.gate_job_tool_call_for_test(job, "mkdir build");
    app.clamp_text_panes(100, 30); // the next frame
    assert!(page(&app).contains("needs you"), "{}", page(&app));
    assert_eq!(
        app.text_pane_pos_for_test(pane).expect("a text page").0,
        place,
        "the page changed under you and kept your place"
    );
}

/// **An id typed off its border is that id, all digits or not** — `:b`
/// and eight characters of an id that begins with eight digits. A bare
/// number is `b<N>` only while it can be a buffer's (`as_listed`); every
/// one became `b<N>`, and `b` and eight digits read as an id beginning with
/// `b`, so about one id in forty-three could not be typed to `:b` — it
/// flaked `ctrl_hat_to_a_session_moves_head_as_b_n_does` about one run in
/// twenty. The id is FOUND, not hard-coded (`from_seed` is a hash, so the
/// same one every run, after a few dozen seeds).
#[test]
fn an_id_of_digits_typed_off_its_border_is_that_id() {
    let id = (0u32..)
        .map(|i| shannon_core::EpisodeId::from_seed(format!("digits {i}").as_bytes()))
        .find(|id| id.to_hex()[..8].chars().all(|c| c.is_ascii_digit()))
        .expect("the search is unbounded");
    let mut app = test_app();
    let recall = app.unlock_turns(&[]).expect("nothing to unlock");
    let root = shannon_core::Episode::root(
        id,
        shannon_core::TurnConfig {
            model: "stub".to_string(),
            max_tokens: None,
            effort: None,
        },
    );
    let at = app.current;
    app.adopt_session(at, root, Vec::new(), OnDisk::Absent, recall, Landing::Asked);
    app.conversation_mut().begin_turn("first");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    app.new_session_command();
    assert_ne!(app.conversation().episode().id, id, "premise: left it");
    app.dispatch_command_for_test(&format!("b {}", &id.to_hex()[..8]));
    assert_eq!(
        app.conversation().episode().id,
        id,
        "`:b` and the eight digits on its border: {:?}",
        app.notice()
    );
    assert_eq!(
        as_listed("123456"),
        "b123456",
        "six digits: a buffer's number"
    );
    assert_eq!(as_listed("1234567"), "1234567", "seven: no buffer's, an id");
}

/// **A prefix two sessions share is refused by every verb that takes a
/// session**, naming both as `:ls` does — never resolved to whichever came
/// first. `:open`, `:clone` and `:merge` took the first match, and the only
/// test of ambiguity fed `""`, which no verb can produce (Reviewers A, C,
/// round 16).
///
/// **Every verb, from the table** (review stop 12, T4 2): the hand list
/// named six of twelve, and `:tabnew` taking the first match passed the
/// whole suite. And nothing moves — what each verb could move, in one
/// probe: a refused `:Send` keeps the prompt's text, a refused `:tabnew`
/// opens no tab, a refused `:rename` names nothing.
#[tokio::test]
async fn a_prefix_two_sessions_share_is_refused_by_every_verb() {
    let (mut app, a, b) = app_with_colliding_sessions();
    let short = shannon_core::SHORT_ID_LEN;
    let prefix = a.to_hex()[..short].to_string();
    assert_eq!(b.to_hex()[..short], prefix, "premise: they collide");
    app.prompt_mut().insert_pasted("hello");
    let probe = |app: &App| {
        (
            (app.conversation().episode().id, app.prompt_target()),
            (app.prompt().text(), app.workspace.focused()),
            (app.tab_labels(), app.buffers.sessions().count()),
            (app.generations.len(), app.pending.len()),
            app.pending_confirm.is_empty(),
            (app.session_label(a), app.session_label(b)),
        )
    };
    let before = probe(&app);
    // The words a verb reads after the session — `:rename`'s alone, whose
    // one word is the NEW name.
    let tail = |verb: &str| match verb {
        "rename" => " a-new-name",
        _ => "",
    };
    let verbs: Vec<&str> = (crate::command::TABLE.iter())
        .filter(|spec| spec.arg.ids != crate::command::IdSpace::None)
        .map(|spec| spec.name)
        .collect();
    assert!(
        ["Send", "open", "tabnew", "rename"]
            .iter()
            .all(|v| verbs.contains(v)),
        "premise: the table's session verbs: {verbs:?}"
    );
    for verb in &verbs {
        app.notices.clear();
        app.dispatch_command_for_test(&format!("{verb} {prefix}{}", tail(verb)));
        let notice = app.notice().unwrap_or_default();
        assert!(notice.contains("names 2"), ":{verb} {prefix}: {notice:?}");
        assert!(
            notice.contains("b1 ") && notice.contains("b2 "),
            "named as `:ls` names them: {notice:?}"
        );
        assert_eq!(probe(&app), before, ":{verb} {prefix} moved something");
    }

    // Enough of the id names one.
    let longer = a.to_hex()[..16].to_string();
    assert!(!b.to_hex().starts_with(&longer), "premise: unique at 16");
    app.dispatch_command_for_test(&format!("delete {longer}"));
    match app.pending_confirm.front() {
        Some(ConfirmAction::Delete { episodes, .. }) => assert_eq!(episodes, &vec![a]),
        other => panic!("expected a delete of {a:?}: {other:?}"),
    }
}

/// **A `:Send` list is read whole before anything acts on it**: a bad name
/// at its end loaded the session its first name named into `:ls`, then
/// refused (Reviewer B, round 16). And `b` with too many digits names no
/// buffer — it overflowed, and was read as a session id instead.
#[tokio::test]
async fn a_send_list_is_read_whole_before_any_session_loads() {
    let dir = TempDir::new().unwrap();
    let stored = {
        let mut store = JsonlStore::new(dir.path()).unwrap();
        let mut c = Conversation::with_config(TurnConfig {
            model: "stub".into(),
            max_tokens: None,
            effort: None,
        });
        c.begin_turn("stored");
        c.push_chunk("reply");
        c.commit_turn();
        store.put_episode(c.episode()).unwrap();
        for t in c.own_turns() {
            store.append_turn(&c.episode().id, &t).unwrap();
        }
        c.episode().id
    };
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    let open = |app: &App| {
        app.buffers
            .sessions()
            .any(|b| b.conversation.episode().id == stored)
    };
    assert!(!open(&app), "premise: stored, not open");
    let id = stored.to_hex()[..8].to_string();

    app.prompt_mut().insert_pasted("hello");
    app.dispatch_command_for_test(&format!("Send {id},bogus"));
    assert!(
        app.notice().unwrap_or_default().contains("bogus"),
        "{:?}",
        app.notice()
    );
    assert!(!open(&app), "a refused list loaded a session");
    assert_eq!(
        app.prompt().text(),
        "hello",
        "a refused send keeps the text"
    );

    // The control: the same name alone loads it.
    app.dispatch_command_for_test(&format!("Send {id}"));
    assert!(open(&app), "premise: the name loads it: {:?}", app.notice());

    // A buffer number is at most six digits; past that `b…` is an id, as a
    // short id read off a border must be (Steven, 10-20) — and either way
    // the refusal says which reading it took.
    let most = "b999999";
    app.dispatch_command_for_test(&format!("buffer {most}"));
    assert!(
        app.notice()
            .unwrap_or_default()
            .contains(&format!("there is no buffer {most}")),
        "{:?}",
        app.notice()
    );
    let huge = "b99999999999999999999";
    app.dispatch_command_for_test(&format!("buffer {huge}"));
    assert!(
        app.notice()
            .unwrap_or_default()
            .contains(&format!("no session matches `{huge}`")),
        "{:?}",
        app.notice()
    );
}

/// **`b<N>` names the buffer `:ls` numbers N, in every verb** — `:delete b1`
/// read `b1` as an id PREFIX and deleted whichever session's id began so,
/// under a confirmation that echoed `b1` back (Reviewer A, round 16). The
/// confirmation names it as `:ls` does; a prefix too short (`3`) is
/// refused; an id is matched whatever its case.
#[tokio::test]
async fn b_n_names_the_buffer_ls_numbers_in_every_verb() {
    let (mut app, a, b) = app_with_colliding_sessions();
    app.dispatch_command_for_test("delete b1");
    match app.pending_confirm.front() {
        Some(ConfirmAction::Delete { episodes, .. }) => assert_eq!(episodes, &vec![a]),
        other => panic!("`:delete b1` should confirm b1's session: {other:?}"),
    }
    let question = app.confirm_prompt().expect("asked").text();
    assert!(
        question.contains(&format!("b1 {}", &a.to_hex()[..8])),
        "{question}"
    );
    app.pending_confirm.clear();

    app.dispatch_command_for_test("delete 3");
    assert!(
        app.notice().unwrap_or_default().contains("too short"),
        "{:?}",
        app.notice()
    );
    assert!(
        app.pending_confirm.is_empty(),
        "a short prefix deletes nothing"
    );

    app.dispatch_command_for_test("buffer b1");
    assert_eq!(app.conversation().episode().id, a, "`:buffer b1`");
    app.dispatch_command_for_test(&format!("open {}", b.to_hex()[..16].to_uppercase()));
    assert_eq!(app.conversation().episode().id, b, "an id in capitals");
}

/// **What a finished command left running is work quitting would end**:
/// `:qa` names it and refuses, `:qa!` ends it (Steven, 2026-09-30: "it
/// should kill the process"). It used to be released with its shell: no
/// question, and it outlived Shannon.
#[test]
fn quitting_names_what_finished_commands_left_running_and_bang_ends_it() {
    use std::sync::atomic::Ordering;
    let (mut app, exec) = app_with_a_leftover(0);
    assert!(!app.dispatch_command_for_test("qa"), "`:qa` left");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("`python3 -m http.server &` left python3 (pid 4242) running")
            && notice.contains("`:qa!`"),
        "{notice:?}"
    );
    assert!(
        !exec.ended.load(Ordering::SeqCst),
        "a refused quit ends nothing"
    );
    assert!(app.dispatch_command_for_test("qa!"), "`:qa!` leaves");
    assert!(exec.ended.load(Ordering::SeqCst), "ended on the way out");
}

/// **What holds the exit is said where you reach for the exit** (Reviewer F,
/// round 17): CTRL-C with nothing it stops said "type :qa", and `:qa` then
/// refused over a leftover; the refusal put its remedy last, where a narrow
/// row cut it, under "still running:" for things that do not run.
#[tokio::test]
async fn ctrl_c_and_quit_name_what_holds_the_exit_remedy_first() {
    let mut app = test_app();
    app.interrupt();
    assert!(
        app.notice().unwrap_or_default().contains("type  :qa"),
        "premise: idle, the plain answer"
    );

    let (mut app, _) = app_with_a_leftover(0);
    app.interrupt();
    let notice = app.notice().unwrap_or_default();
    // The quit refusal's words, remedy first: "`:qa!` ends it" was false of
    // an unwritten session, which it drops (review stop 2, A F7).
    assert!(
        notice.contains("`:qa!` quits anyway; quitting would end:")
            && notice.contains("python3 (pid 4242)"),
        "{notice:?}"
    );
    assert!(!app.dispatch_command_for_test("qa"), "premise: refused");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.starts_with("add `!` to quit anyway") && notice.contains("quitting would end:"),
        "{notice:?}"
    );
}

/// `:ls` says what is actually running once its command has finished, and
/// how much it has printed since; with nothing left, it says nothing.
#[test]
fn ls_lists_what_finished_commands_left_running() {
    let (app, _) = app_with_a_leftover(2048);
    let listing = app.buffers_listing(false);
    assert!(
        listing.contains("Left running by finished commands")
            && listing.contains("python3 (pid 4242)")
            && listing.contains("`python3 -m http.server &`")
            && listing.contains("printed 2 KB since it finished"),
        "{listing}"
    );
    assert!(
        !test_app().buffers_listing(false).contains("Left running"),
        "no section when nothing is left"
    );
}

/// **What a leftover said last, and whose call started it.** The line is
/// the leftover's own output, so it is untrusted: no control character
/// reaches the screen (an escape sequence there could retitle the window
/// or move the cursor). Its secrets are the executor's to redact, as for
/// any output (`the_last_line_is_redacted_once_per_change`, core); this
/// fake hands over a line already redacted.
///
/// And "whose call" is the session that ASKED, even when its result arrives
/// with another in front: the attribution was read off `current` at the
/// outcome, and the test fed a call no session had made, so it asserted the
/// fallback (Reviewers C, D, round 17).
#[tokio::test]
async fn ls_says_whose_call_left_it_and_what_it_said_last() {
    let said = "\x1b]0;pwned\x07token {{gh.4f19}} \x1b[31mready";
    let (mut app, _) = with_a_leftover(test_app(), 64, Some(said));
    let listing = app.buffers_listing(false);
    let last = listing
        .lines()
        .find(|l| l.trim_start().starts_with("last:"))
        .unwrap_or_else(|| panic!("a last line: {listing}"));
    assert!(!last.contains('\x1b') && !last.contains('\x07'), "{last:?}");
    assert!(
        last.contains("{{gh.4f19}}") && last.contains("ready"),
        "{last:?}"
    );
    assert!(
        !listing.contains("from b"),
        "unknown until its result arrives"
    );

    // Asked in this session; its result arrives with another in front, and
    // says whose call it was.
    let asker = app.conversation().episode().id;
    app.track_tool_call("toolu_srv", None);
    // A committed turn, so `:new` really leaves it (an empty session is
    // reused).
    app.conversation_mut().begin_turn("start the server");
    app.conversation_mut().push_chunk("started");
    app.conversation_mut().commit_turn();
    app.new_session_command();
    assert_ne!(
        app.conversation().episode().id,
        asker,
        "premise: another session in front"
    );
    app.handle_exec_outcome(shannon_core::ExecOutcome {
        tool_use_id: "toolu_srv".to_string(),
        command: "python3 -m http.server &".to_string(),
        output: String::new(),
        code: Some(0),
        timed_out: false,
        truncated: false,
        spawn_error: None,
        redacted: Vec::new(),
        spent: Vec::new(),
        attempts: 1,
        kept: None,
        left: shannon_core::LeftBehind::Held(vec![shannon_core::exec::LeftProcess {
            pid: 4242,
            name: "python3".into(),
        }]),
        file: None,
    });
    let listing = app.buffers_listing(false);
    assert!(
        listing.contains(&format!("from {} · ", app.session_label(asker))),
        "{listing}"
    );
}

/// **vim's `:b1` and `:2b`** reach the buffer `:b 1` does, through dispatch
/// — and a number given twice, or a count on a command that takes none, is
/// refused rather than guessed at.
#[test]
fn buffer_takes_vims_glued_and_counted_forms() {
    for form in ["b1", "1b", "bu 1", "buf1", "buffe 1", "1buffer"] {
        let mut app = test_app();
        two_sessions_b1_hidden(&mut app);
        app.dispatch_command_for_test(form);
        assert_eq!(app.current, BufferId(0), "`:{form}` shows b1");
    }
    let mut app = test_app();
    two_sessions_b1_hidden(&mut app);
    app.notices.clear();
    app.dispatch_command_for_test("2b 1");
    assert_eq!(app.current, BufferId(1), "`:2b 1` moved nothing");
    assert!(
        app.notice().unwrap_or_default().contains("not both"),
        "{:?}",
        app.notice()
    );
    app.notices.clear();
    app.dispatch_command_for_test("3q");
    assert!(
        app.notice()
            .unwrap_or_default()
            .contains("no count allowed: `:3q`"),
        "{:?}",
        app.notice()
    );
}

/// **`:Send ball` — vim's `:ball`, every buffer**: each session `:ls` lists,
/// the hidden one included. Steven reached for it and it named nothing.
#[tokio::test]
async fn send_ball_reaches_every_session_shown_or_not() {
    let mut app = test_app();
    two_sessions_b1_hidden(&mut app);
    app.prompt_mut().insert_pasted("to everyone");
    app.dispatch_command_for_test("Send ball");
    for b in [BufferId(0), BufferId(1)] {
        assert!(
            app.generations.iter().any(|g| g.buffer == b),
            "{b:?} got it: {:?}",
            app.notice()
        );
    }
    assert!(
        app.notice()
            .unwrap_or_default()
            .contains("sent to 2 sessions"),
        "{:?}",
        app.notice()
    );
    assert!(app.prompt().text().is_empty(), "the prompt was consumed");
}

/// **What a session never written holds of its own** (S21-Q12) — one made
/// as a look (a checkout, a clone) holds nothing until given something; any
/// other is a composition (an edit's prune, a merge, a pick), what you did,
/// owning no turn as it may, whatever its origin's kind: a `Branch` an edit
/// made was read as a look by its kind and quit unasked (the close
/// verifier, HIGH 1). An empty list of marks given a look is no mark.
#[test]
fn a_composition_is_something_of_its_own_and_a_look_is_not() {
    use shannon_core::{EdgeKind, Episode, EpisodeId, Message, Origin, TurnConfig, TurnId};
    let config = TurnConfig {
        model: "stub".into(),
        max_tokens: None,
        effort: None,
    };
    for kind in [EdgeKind::Clone, EdgeKind::Branch, EdgeKind::Merge] {
        let origin = Origin {
            episode: EpisodeId::from_seed(b"source"),
            turn: TurnId::compute(&[Message::user("a")], &config),
            index: Some(0),
            kind,
        };
        let made = Episode::from_segments(
            EpisodeId::from_seed(b"made"),
            config.clone(),
            Vec::new(),
            Some(origin),
        );
        let contents = super::buffers::Contents {
            conversation: shannon_core::Conversation::resume(made, Vec::new()),
            output: crate::output::Output::new(),
            on_disk: OnDisk::Absent,
            billing: Billing::default(),
        };
        let mut buffer = SessionBuffer::new(BufferId(7), contents, super::Landing::Asked);
        assert!(
            buffer.has_its_own(),
            "{kind:?}: a composition, unless made as a look"
        );
        buffer.made_as_a_look();
        assert!(!buffer.has_its_own(), "{kind:?}: a look");
        buffer.conversation.set_marks(Vec::new());
        assert!(!buffer.has_its_own(), "{kind:?}: no marks is no mark");
    }
}
