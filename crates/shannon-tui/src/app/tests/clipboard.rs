//! The clipboard: OSC 52, yank-only sync, pasted secrets.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

// --- clipboard (OSC 52, yank-only sync) ---------------------------

#[test]
fn yank_mirrors_to_the_system_clipboard() {
    let mut app = test_app();
    let clip = MemoryClipboard::default();
    app.osc = Box::new(clip.clone());
    type_into(&mut app, "hello world");
    app.enforce_normal_cursor();
    type_keys(&mut app, "0"); // to col 0
    type_keys(&mut app, "y"); // operator pending
    type_keys(&mut app, "w"); // yw → "hello "
    assert_eq!(clip.last().as_deref(), Some("hello "));
}

/// **`tui.copy_on_yank = false` stops a `y` at the register** — vim's own
/// default, without `clipboard=unnamedplus` (Steven, 11-09, for the tutor's
/// note on the clipboard): nothing sent, `p`'s register holds it, and a yank
/// of more than two lines is still counted, as vim's 'report' does.
#[test]
fn a_yank_with_copy_on_yank_off_stays_in_the_register() {
    let mut app = normal_at(&["one", "two", "three"], 0, 0);
    let mut config = (*app.config).clone();
    config.tui.copy_on_yank = false;
    let _ = app.adopt_config(config);
    let clip = MemoryClipboard::default();
    app.osc = Box::new(clip.clone());
    for key in ["y", "w"] {
        type_keys(&mut app, key);
    }
    assert_eq!(clip.count(), 0, "nothing sent");
    let held = app.prompt().register_copied().map(|c| c.text());
    assert_eq!(held.as_deref(), Some("one"), "the register has it");
    for key in ["3", "y", "y"] {
        type_keys(&mut app, key);
    }
    assert_eq!(clip.count(), 0, "nothing sent");
    assert!(
        app.notice().is_some_and(|n| n.contains("3 lines yanked")),
        "{:?}",
        app.notice()
    );
}

/// **A yank on a plain test app never reaches the real clipboard** — the
/// `MemoryClipboard` `sandboxed` installs takes it (and the real sink refuses
/// under `cfg(test)`). Deleting that line passed every test before this one
/// (review of d493060, Reviewers B and C).
#[test]
fn a_test_apps_yank_stays_in_memory() {
    let mut app = test_app();
    type_into(&mut app, "hello world");
    app.enforce_normal_cursor();
    type_keys(&mut app, "0");
    type_keys(&mut app, "y");
    type_keys(&mut app, "w");
}

#[test]
fn delete_and_x_do_not_touch_the_system_clipboard() {
    // Cuts fill the internal register (for `p`) but must not clobber the
    // OS clipboard — only an explicit yank syncs.
    let mut app = test_app();
    let clip = MemoryClipboard::default();
    app.osc = Box::new(clip.clone());
    type_into(&mut app, "abc");
    app.enforce_normal_cursor();
    type_keys(&mut app, "x"); // delete a char
    type_keys(&mut app, "d");
    type_keys(&mut app, "d"); // dd
    assert_eq!(clip.count(), 0, "no OS-clipboard writes from d/x");
}

#[test]
fn visual_yank_mirrors_to_the_system_clipboard() {
    let mut app = test_app();
    let clip = MemoryClipboard::default();
    app.osc = Box::new(clip.clone());
    type_into(&mut app, "abc");
    app.enforce_normal_cursor();
    type_keys(&mut app, "0"); // (0,0)
    type_keys(&mut app, "v"); // Visual, anchor (0,0)
    type_keys(&mut app, "l"); // extend to (0,1)
    type_keys(&mut app, "y"); // yank "ab"
    assert_eq!(clip.last().as_deref(), Some("ab"));
}

/// **Copying a selection sends it and keeps it** — what a drag's release
/// does under `tui.copy_on_select` (demo, 2026-10-02: a drag is Shannon's
/// selection, so Cmd-C after it copied nothing; off by default since
/// 10-06, Steven). The selection stays up and `p`'s register is untouched —
/// vim's `clipboard=autoselect`. With nothing selected, nothing is written.
/// Called directly: whether a release calls it is the drag tests'.
#[test]
fn copying_a_selection_sends_it_and_keeps_it() {
    let mut app = test_app();
    let clip = MemoryClipboard::default();
    app.osc = Box::new(clip.clone());
    type_into(&mut app, "abc");
    app.enforce_normal_cursor();
    type_keys(&mut app, "0");
    app.copy_selection();
    assert_eq!(clip.count(), 0, "no selection, no copy");

    type_keys(&mut app, "v");
    type_keys(&mut app, "l"); // "ab"
    app.copy_selection();
    assert_eq!(clip.last().as_deref(), Some("ab"));
    assert!(
        app.notice()
            .unwrap_or_default()
            .contains("2 characters sent to the clipboard"),
        "{:?}",
        app.notice()
    );
    assert!(app.focused_is_visual(), "the selection stays up");
    assert_eq!(
        app.prompt().register_text().as_deref(),
        None,
        "`p`'s register untouched"
    );

    // A selection over a marked secret says which value went with it.
    let handle = app.mark_secret("ab", true).expect("marked");
    app.copy_selection();
    assert!(
        app.notice()
            .is_some_and(|n| n.contains(&format!("with the value of {handle}"))),
        "{:?}",
        app.notice()
    );
}

/// **A drag that nothing copies says what will** (review stop 17, F M3): the
/// reflex is Cmd-C, and it copies nothing. Only a drag, and only one that
/// selected something: keyboard Visual stays silent, as vim's does.
#[tokio::test]
async fn a_drag_says_what_copies_it() {
    let mut app = test_app();
    assert!(!app.config.tui.copy_on_select, "premise: the default");
    app.mouse_dragged = true;
    app.release_left();
    assert!(
        app.notice().is_none(),
        "nothing selected: {:?}",
        app.notice()
    );

    type_into(&mut app, "abc");
    app.enforce_normal_cursor();
    type_keys(&mut app, "0");
    type_keys(&mut app, "v");
    type_keys(&mut app, "l");
    app.notices.clear();
    app.release_left();
    assert!(app.notice().is_none(), "a click: {:?}", app.notice());
    app.mouse_dragged = true;
    app.release_left();
    let notice = app.notice().unwrap_or_default();
    assert!(notice.contains("`y` copies it"), "{notice:?}");
}

/// **A release copies only what a drag selected — in either text pane, and
/// only with `tui.copy_on_select`.** The rule lives in `release_left`, which
/// the mouse's Up arm calls; a drag sets `mouse_dragged` (set by hand here —
/// `handle_mouse` takes a live terminal). The transcript half is fix 51's own
/// demo, and nothing held it (Reviewer C), nor the setting (Reviewer A).
#[tokio::test]
async fn only_a_drag_release_copies_in_either_pane() {
    let clip = MemoryClipboard::default();
    let mut on = Config::default();
    on.tui.copy_on_select = true;
    let mut app = test_app_with(on);
    app.osc = Box::new(clip.clone());
    app.submit("hello there");
    app.feed_stream(StreamMsg::Chunk("general kenobi".into()));
    app.feed_stream(StreamMsg::Complete);
    type_into(&mut app, "abc");
    app.enforce_normal_cursor();
    type_keys(&mut app, "0");
    type_keys(&mut app, "v");
    type_keys(&mut app, "l");

    app.release_left();
    assert_eq!(
        clip.count(),
        0,
        "a click selected nothing, so nothing is copied"
    );
    app.mouse_dragged = true;
    app.release_left();
    assert_eq!(clip.last().as_deref(), Some("ab"), "the prompt's drag");
    assert!(!app.mouse_dragged, "the gesture ends with the release");

    app.set_focus(PaneKind::Output);
    app.before_frame(80, 24); // the chat buffer syncs before the next frame
    // Through the focused pane's keymap (`press` drives the prompt's only).
    // From the echo, below the first line's rule: a transcript is entered at
    // its end, where its window is, on the blank line closing the last turn.
    for code in "ggjvl".chars().map(KeyCode::Char) {
        app.key_for_test(KeyEvent::new(code, KeyModifiers::NONE), (80, 20));
    }
    assert!(
        app.focused_is_visual(),
        "premise: a selection in the transcript"
    );
    app.mouse_dragged = true;
    app.release_left();
    assert_eq!(clip.count(), 2, "the transcript's drag copies too");
    assert!(
        clip.last().is_some_and(|t| !t.is_empty() && t != "ab"),
        "{:?}",
        clip.last()
    );

    // Off by DEFAULT (Steven, 10-06: "disable by default"): a drag selects,
    // and `y` copies.
    assert!(!Config::default().tui.copy_on_select);
    let clip = MemoryClipboard::default();
    let mut app = test_app();
    app.osc = Box::new(clip.clone());
    type_into(&mut app, "abc");
    app.enforce_normal_cursor();
    type_keys(&mut app, "v");
    app.mouse_dragged = true;
    app.release_left();
    assert_eq!(clip.count(), 0, "by default a release copies nothing");
}

/// **A copy that touches a secret goes as `tui.copy_secrets` says** (Steven,
/// 2026-10-02: "by default copy as plain text"), and **half a key is the
/// key**: the drag below takes `pw=hunt`, which holds no secret as text —
/// the old check (`text.contains(value)`) said nothing about it.
#[test]
fn a_drag_over_part_of_a_secret_copies_as_the_option_says() {
    use shannon_core::CopySecrets;
    let drag = |app: &mut App| {
        app.mouse_dragged = true;
        app.release_left();
    };

    let (mut app, clip, handle) = copying(CopySecrets::Value);
    drag(&mut app);
    assert_eq!(
        clip.last().as_deref(),
        Some("pw=hunt"),
        "the default: as shown"
    );
    assert!(
        app.notice()
            .is_some_and(|n| n.contains(&format!("with the value of {handle}"))),
        "{:?}",
        app.notice()
    );

    let (mut app, clip, handle) = copying(CopySecrets::Handle);
    drag(&mut app);
    assert_eq!(clip.last(), Some(format!("pw={handle}")));
    assert!(
        app.notice()
            .is_some_and(|n| n.contains(&format!("{handle} as its handle, not its value"))),
        "{:?}",
        app.notice()
    );

    let (mut app, clip, handle) = copying(CopySecrets::Skip);
    drag(&mut app);
    assert_eq!(clip.count(), 0, "nothing sent");
    assert_eq!(app.notice_kind(), Some(crate::app::NoticeKind::Refusal));
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("nothing sent to the clipboard")
                && n.contains(&handle)
                && n.contains("copy_secrets")),
        "{:?}",
        app.notice()
    );
    // A selection that only abuts the secret copies under any setting.
    type_keys(&mut app, "0");
    type_keys(&mut app, "l");
    type_keys(&mut app, "l");
    drag(&mut app);
    assert_eq!(clip.last().as_deref(), Some("pw="), "{:?}", app.notice());
}

/// **A `y` copies through the same door** — and `p`'s register keeps what the
/// screen shows. A `y` is silent as vim's is, unless the clipboard got
/// something other than what the screen shows — or it took part of a held
/// value, which is marked so that a paste of it leaves as a handle (review
/// stop 14, E F4: the register "never leaves Shannon" was false once pasted
/// and sent).
#[test]
fn a_prompt_yank_mirrors_as_the_option_says_and_keeps_the_register() {
    use shannon_core::CopySecrets;
    let yank = |app: &mut App| type_keys(app, "y");

    let (mut app, clip, _) = copying(CopySecrets::Value);
    app.notices.clear();
    yank(&mut app);
    assert_eq!(clip.last().as_deref(), Some("pw=hunt"));
    // Nothing but the part's marking — "no notice saying `yanked`" passed a
    // `y` that spoke in other words (Reviewer C, round 19).
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with("part of {{") && !said.contains(" · "),
        "the part, alone: {said}"
    );

    let (mut app, clip, handle) = copying(CopySecrets::Handle);
    yank(&mut app);
    assert_eq!(clip.last(), Some(format!("pw={handle}")));
    assert_eq!(
        app.prompt().register_text().as_deref(),
        Some("pw=hunt"),
        "`p` as shown"
    );
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("yanked") && n.contains(&handle)),
        "{:?}",
        app.notice()
    );

    let (mut app, clip, handle) = copying(CopySecrets::Skip);
    yank(&mut app);
    assert_eq!(clip.count(), 0, "nothing sent");
    assert_eq!(
        app.prompt().register_text().as_deref(),
        Some("pw=hunt"),
        "still yanked"
    );
    assert_eq!(app.notice_kind(), Some(crate::app::NoticeKind::Refusal));
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("yanked for `p`") && n.contains(&handle)),
        "{:?}",
        app.notice()
    );
}

/// The transcript's `y` too: a secret shown in the chat is judged the same.
#[tokio::test]
async fn a_chat_yank_goes_through_the_same_door() {
    use shannon_core::CopySecrets;
    let mut cfg = Config::default();
    cfg.tui.copy_secrets = CopySecrets::Skip;
    let mut app = test_app_with(cfg);
    let clip = MemoryClipboard::default();
    app.osc = Box::new(clip.clone());
    let handle = app.mark_secret("hunter2", true).expect("marked");
    app.submit("use hunter2 now");
    app.feed_stream(StreamMsg::Chunk("ok".into()));
    app.feed_stream(StreamMsg::Complete);
    app.set_focus(PaneKind::Output);
    app.before_frame(80, 24);
    for code in ['g', 'g', 'V', 'G', 'y'] {
        app.key_for_test(
            KeyEvent::new(KeyCode::Char(code), KeyModifiers::NONE),
            (80, 20),
        );
    }
    assert!(
        app.prompt()
            .register_text()
            .is_some_and(|t| t.contains("hunter2")),
        "premise: the chat shows the value, and `p` gets it: {:?}",
        app.prompt().register_text().as_deref()
    );
    assert_eq!(clip.count(), 0, "{:?}", app.notice());
    assert!(
        app.notice().is_some_and(|n| n.contains(&handle)),
        "{:?}",
        app.notice()
    );
}

/// **The transcript's `viw` and `o`** — a text object, and a swap of the
/// selection's ends — which its own arm dropped silently; now through the
/// read-only buffer's one door (`Buffer::apply_read_only`), which the pages
/// use too.
#[tokio::test]
async fn the_transcript_takes_a_text_object_and_o() {
    let mut app = test_app();
    app.osc = Box::new(MemoryClipboard::default());
    app.submit("alpha beta gamma");
    app.feed_stream(StreamMsg::Chunk("ok".into()));
    app.feed_stream(StreamMsg::Complete);
    app.set_focus(PaneKind::Output);
    app.before_frame(80, 24);
    let (row, col) = app
        .chat_mut()
        .unwrap()
        .lines()
        .enumerate()
        .find_map(|(i, l)| l.find("beta").map(|b| (i, l[..b].chars().count())))
        .expect("the message is in the transcript");
    app.chat_mut().unwrap().place_cursor(row, col + 1);
    type_keys(&mut app, "viwy");
    assert_eq!(app.prompt().register_text().as_deref(), Some("beta"));
    app.chat_mut().unwrap().place_cursor(row, col);
    type_keys(&mut app, "vllo");
    assert_eq!(
        app.chat_mut().unwrap().cursor(),
        (row, col),
        "`o`: to the anchor"
    );
}

/// The chat's `y` under the default (`value`): it copies what the screen
/// shows and says nothing, as the prompt's does — the silence asserted as an
/// absence, not as "no notice containing a word".
#[tokio::test]
async fn a_chat_yank_of_a_secret_is_silent_by_default() {
    let mut app = test_app();
    let clip = MemoryClipboard::default();
    app.osc = Box::new(clip.clone());
    app.mark_secret("hunter2", true).expect("marked");
    app.submit("use hunter2 now");
    app.feed_stream(StreamMsg::Chunk("ok".into()));
    app.feed_stream(StreamMsg::Complete);
    app.set_focus(PaneKind::Output);
    app.before_frame(80, 24);
    app.notices.clear();
    // The first line is the rule above your message; the echo is below it.
    for code in ['g', 'g', 'j', 'y', 'y'] {
        app.key_for_test(
            KeyEvent::new(KeyCode::Char(code), KeyModifiers::NONE),
            (80, 20),
        );
    }
    assert!(
        clip.last().is_some_and(|t| t.contains("hunter2")),
        "{:?}",
        clip.last()
    );
    assert_eq!(app.notice(), None, "silent, as vim");
}

/// **Part of a held value, copied, leaves as a secret** (review stop 14,
/// E F4 — E's probe): `v7ly` over `hunter2beornot2be`, `p`, submit sent
/// `"hunter2b"`, unstyled and unsaid. The part is marked when copied, as a
/// secret of its own, and said; sent, it goes as its handle and the
/// withheld count names it. A part too short to mark is said too.
#[tokio::test]
async fn part_of_a_value_copied_leaves_as_a_secret() {
    let mut app = test_app();
    app.mark_secret("hunter2beornot2be", true).expect("marked");
    app.submit("the key is hunter2beornot2be");
    app.feed_stream(StreamMsg::Chunk("ok".into()));
    app.feed_stream(StreamMsg::Complete);
    app.set_focus(PaneKind::Output);
    app.before_frame(80, 24);
    let (row, col) = (app.chat_mut().unwrap().lines().enumerate())
        .find_map(|(i, l)| l.find("hunter2b").map(|b| (i, l[..b].chars().count())))
        .expect("the value is in the transcript");
    app.chat_mut().unwrap().place_cursor(row, col);
    app.notices.clear();
    type_keys(&mut app, "v7ly");
    let taken = app.prompt().register_text().unwrap_or_default();
    assert_eq!(taken, "hunter2b", "premise: the part taken");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("part of {{") && said.contains("marked too, as {{"),
        "{said}"
    );
    app.notices.clear();
    app.submit(&format!("try {taken}"));
    let sent = (app.conversation().messages().iter())
        .rev()
        .find(|m| m.role == shannon_core::Role::User)
        .map(|m| m.content.clone())
        .unwrap_or_default();
    assert!(!sent.contains("hunter2b"), "the part in clear: {sent}");
    assert!(sent.contains("{{"), "as its handle: {sent}");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("1 secret withheld"), "{said}");
    // Spendable as its value is: the part of a lent value is lent.
    let part = (app.aliases().secrets()).find(|a| a.name.ends_with("-part"));
    assert!(part.is_some_and(|a| a.spendable), "{part:?}");
    // The run's alone, never kept for later runs where its value is (the
    // close's verifier, V 6: a part of a lent value landed in the vault).
    let part = part.map(|a| a.handle.clone()).unwrap_or_default();
    let whole = (app.aliases().secret_holding("hunter2beornot2be")).map(|a| a.handle.clone());
    let whole = whole.expect("the value held");
    assert!(
        matches!(app.vault.recall(&whole), Ok(Some(_))),
        "premise: this vault keeps a value"
    );
    assert!(
        matches!(app.vault.recall(&part), Ok(None)),
        "the part is not kept"
    );

    // Three characters: too few to mark, said.
    app.set_focus(PaneKind::Output);
    app.before_frame(80, 24);
    app.chat_mut().unwrap().place_cursor(row, col + 9);
    app.notices.clear();
    type_keys(&mut app, "v2ly");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("copied 3 characters of {{") && said.contains("too few"),
        "{said}"
    );

    // A value kept off the wire (`--no-spend`): its part is too.
    let mut app = test_app();
    app.mark_secret("correct-horse-battery", false)
        .expect("marked");
    app.submit("the phrase is correct-horse-battery");
    app.set_focus(PaneKind::Output);
    app.before_frame(80, 24);
    let (row, col) = (app.chat_mut().unwrap().lines().enumerate())
        .find_map(|(i, l)| l.find("correct").map(|b| (i, l[..b].chars().count())))
        .expect("the value is in the transcript");
    app.chat_mut().unwrap().place_cursor(row, col);
    type_keys(&mut app, "v6ly");
    let part = (app.aliases().secrets()).find(|a| a.name.ends_with("-part"));
    assert!(part.is_some_and(|a| !a.spendable), "{part:?}");
}

/// **A part is counted as it would be marked, trimmed** (the close's
/// verifier, V 5): " vut" counted four and marked `vut`, three — under the
/// floor — and every `vut` you write went as a handle.
#[tokio::test]
async fn a_part_is_counted_trimmed() {
    let mut app = test_app();
    app.mark_secret("zyxw vutsrq-held", true).expect("marked");
    app.submit("it is zyxw vutsrq-held");
    app.set_focus(PaneKind::Output);
    app.before_frame(80, 24);
    let (row, col) = (app.chat_mut().unwrap().lines().enumerate())
        .find_map(|(i, l)| l.find(" vut").map(|b| (i, l[..b].chars().count())))
        .expect("the value is in the transcript");
    app.chat_mut().unwrap().place_cursor(row, col);
    app.notices.clear();
    type_keys(&mut app, "v3ly");
    assert_eq!(
        app.prompt().register_text().as_deref(),
        Some(" vut"),
        "premise"
    );
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("copied 3 characters of {{"), "{said}");
    let part = (app.aliases().secrets()).find(|a| a.name.ends_with("-part"));
    assert!(part.is_none(), "not marked: {part:?}");
}

/// **The premise of copying a value by default: the model cannot read the
/// clipboard unasked.** A command that prints it is unmodeled, and an
/// unmodeled program always asks. Should one ever be modeled as a plain
/// read, `tui.copy_secrets`'s default needs revisiting — this fails first.
///
/// **Nor the screen, nor another process's environment** (review stop 14,
/// E F5): a value drawn on screen, or lent into an environment, is read
/// back by these — held, until now, by "an unmodelled program asks" alone,
/// so modelling `ps` or `tmux` as reads would have opened them silently.
#[test]
fn printing_the_clipboard_asks() {
    use shannon_core::permission::Decision;
    let app = test_app();
    let clipboard = ["pbpaste", "xclip -o", "xsel --clipboard", "wl-paste"];
    let screen = [
        "tmux capture-pane -p",
        "tmux capture-pane -p -S -",
        "tmux show-buffer",
        "tmux save-buffer -",
        "tmux list-buffers",
        "osascript -e 'the clipboard'",
        "kitty @ get-text",
        "wezterm cli get-text",
    ];
    let environment = [
        "ps eww",
        "ps auxeww",
        "cat /proc/$PPID/environ",
        "cat /proc/1/environ",
    ];
    for line in clipboard.iter().chain(&screen).chain(&environment) {
        assert_eq!(verdict(&app, line), Decision::Ask, "{line}");
    }
}

#[test]
fn sanitize_paste_normalizes_newlines_and_drops_escapes() {
    // CRLF/CR fold to LF; tabs survive; a bare ESC is dropped (the
    // residual "[31m" is now harmless literal text, not a color code).
    assert_eq!(sanitize_paste("a\r\nb\rc"), "a\nb\nc");
    assert_eq!(sanitize_paste("x\x1b[31my\ttab"), "x[31my\ttab");
}

/// **A paste lands where the keys would** (Steven, 2026-10-03: a regex rule
/// typed by hand, because the `:` line took no paste). The `:` line, while
/// open, ahead of the prompt that has focus behind it; the prompt
/// otherwise; and anywhere else a reason, not silence.
#[test]
fn a_paste_lands_where_the_keys_would() {
    let mut app = test_app();
    app.cmdline.open();
    app.paste("deny re:^rm(\\s|$)\r\n");
    assert_eq!(app.cmdline.line(), Some("deny re:^rm(\\s|$)"));
    assert!(
        app.prompt().text().is_empty(),
        "not into the prompt behind it"
    );

    app.paste("ls\nrm -rf ~");
    assert!(
        app.notice().unwrap_or_default().contains("one line"),
        "{:?}",
        app.notice()
    );
    assert_eq!(app.cmdline.line(), Some("deny re:^rm(\\s|$)"));

    // A paste is an edit: a completion menu open before it is stale, and
    // the next `<Tab>` would cycle it over the pasted text.
    type_command(&mut app, "");
    app.command_input(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert!(
        app.completion_menu().is_some(),
        "the subject: a menu is open"
    );
    app.paste(" x");
    assert!(app.completion_menu().is_none());

    app.cmdline.cancel();
    app.paste("hello");
    assert_eq!(app.prompt().text(), "hello");

    app.toggle_focus();
    app.paste("more");
    assert_eq!(app.prompt().text(), "hello", "the chat takes no text");
    assert!(
        app.notice().unwrap_or_default().contains("takes a paste"),
        "{:?}",
        app.notice()
    );

    // A question takes its answer and nothing else — as its keys do.
    app.toggle_focus();
    app.ask(ConfirmAction::DeleteAll);
    app.paste("y");
    assert_eq!(
        app.prompt().text(),
        "hello",
        "not into the prompt behind it"
    );
    assert_eq!(app.pending_confirm.len(), 1, "and not an answer either");
    assert!(
        app.notice().unwrap_or_default().contains("question"),
        "{:?}",
        app.notice()
    );
}
