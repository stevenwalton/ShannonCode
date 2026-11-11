//! Vim editing in the prompt, chords cancelled above the keymap, and the
//! prompt's start mode and submit.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// **A key eaten above the keymap must not strand a half-typed chord.**
///
/// `<C-w>`, then `Ctrl-C` — the universal cancel, and the key vim's `CTRL-C`
/// cancels a pending command with — then `q`: **the pane closed**, because `q`
/// resolved in `Context::AfterWindow`. On the last pane that is a quit. Same
/// after `Ctrl-L`. Confirmed against the running binary before and after
/// (Reviewer D found it; `tools/drive` settled it).
///
/// `Keymap::pending_chord`'s doc has said since it was written that a handler
/// intercepting keys ahead of a keymap must ask — and asking was all it could
/// do, because there was no verb for a `true` answer. That is the whole shape
/// of this bug: a rule written down with no way to obey it.
///
/// Driven through `translate` on the same keymap the app uses, because
/// `press_for_test` reaches only the keymap and **every arm above it is
/// unreachable from the suite** ([[feedback_mutation_validates_the_path]]) —
/// which is exactly why this survived.
#[test]
fn a_key_eaten_above_the_keymap_cancels_a_pending_chord() {
    let mut app = test_app();

    // Premise: the chord really is armed, and `q` really does resolve as a
    // window command while it is.
    app.chat_keymap
        .translate(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
    assert!(
        app.chat_keymap.pending_chord(),
        "premise: <C-w> arms a chord"
    );
    assert!(
        matches!(
            app.chat_keymap
                .translate(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            KeyOutcome::Window(..)
        ),
        "premise: a plain `q` IS <C-w>q while the prefix is armed"
    );

    // Now the defect: arm it, cancel, and `q` must be a plain `q` again.
    app.chat_keymap
        .translate(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
    app.cancel_chords();
    assert!(!app.chat_keymap.pending_chord(), "the chord is abandoned");
    assert!(
        !matches!(
            app.chat_keymap
                .translate(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            KeyOutcome::Window(..)
        ),
        "and `q` no longer closes a pane"
    );

    // **Both keymaps**, or one left armed fires when focus comes back.
    app.prompt_keymap
        .translate(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
    assert!(
        app.prompt_keymap.pending_chord(),
        "premise: armed there too"
    );
    app.cancel_chords();
    assert!(
        !app.prompt_keymap.pending_chord(),
        "the prompt's keymap is cleared by the same door"
    );
}

/// **…and pressed as a user presses them** — `<C-w>`, the key eaten above
/// the keymap, then `q`: the panes as they were. Through `key_for_test`,
/// which takes the shortcuts as `handle_event` does now (`answer_shortcut`).
/// The test above calls the cancel itself, so deleting both of
/// `handle_event`'s cancels passed the crate (review stop 12, T1 1).
#[test]
fn a_shortcut_between_a_prefix_and_its_key_closes_nothing() {
    let key = |code, mods| KeyEvent::new(code, mods);
    let press = |app: &mut App, keys: &[KeyEvent]| {
        for &k in keys {
            app.key_for_test(k, (80, 24));
        }
    };
    let (ctrl_w, q) = (
        key(KeyCode::Char('w'), KeyModifiers::CONTROL),
        key(KeyCode::Char('q'), KeyModifiers::NONE),
    );
    let two_panes = || {
        let mut app = test_app();
        app.set_focus(PaneKind::Output);
        app.split_command("", SplitDir::Vertical);
        // The `:vsplit` event's end, where the loop sees its focus move.
        app.settle();
        let panes = app.workspace.pane_ids();
        assert!(panes.len() >= 2, "premise: a pane to close");
        (app, panes)
    };
    let (mut app, panes) = two_panes();
    press(&mut app, &[ctrl_w, q]);
    assert_ne!(
        app.workspace.pane_ids(),
        panes,
        "premise: `<C-w>q` closes one"
    );

    for (name, eaten) in [
        ("^C", key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        ("^L", key(KeyCode::Char('l'), KeyModifiers::CONTROL)),
        ("PageDown", key(KeyCode::PageDown, KeyModifiers::NONE)),
    ] {
        let (mut app, panes) = two_panes();
        press(&mut app, &[ctrl_w, eaten, q]);
        assert_eq!(app.workspace.pane_ids(), panes, "`<C-w>` {name} `q`");
    }
}

/// **`~`, `r` and `R` do what nvim does** (Steven, 10-02: "`~` doesn't work
/// to change case", "`r` doesn't replace (nor `R`)") — each row's expected
/// text and cursor is nvim 0.x's own answer to the same keys from the same
/// place (headless, `-u NONE`; `[[reference_nvim_oracle]]`), not a reading
/// of the manual.
#[test]
fn tilde_r_and_replace_mode_match_nvim() {
    type Case<'a> = (
        &'a str,
        &'a [&'a str],
        (usize, usize),
        &'a [&'a str],
        (usize, usize),
    );
    let cases: &[Case] = &[
        // `~`: counted, stops at the line's end, lands after — on the last char at most.
        ("5~", &["abc"], (0, 0), &["ABC"], (0, 2)),
        ("~", &["aBc"], (0, 1), &["abc"], (0, 2)),
        ("~", &["abc"], (0, 2), &["abC"], (0, 2)),
        ("2~", &["éaz"], (0, 0), &["ÉAz"], (0, 2)),
        ("~", &["ßa"], (0, 0), &["ẞa"], (0, 1)),
        ("~", &[""], (0, 0), &[""], (0, 0)),
        // `r`: all or nothing, lands on the last replaced; `<CR>` is one break.
        ("5ra", &["abc"], (0, 0), &["abc"], (0, 0)),
        ("3ra", &["abcd"], (0, 0), &["aaad"], (0, 2)),
        ("ra", &["abc"], (0, 2), &["aba"], (0, 2)),
        ("2r<CR>", &["abcd"], (0, 1), &["a", "d"], (1, 0)),
        ("r<CR>", &["abcd"], (0, 1), &["a", "cd"], (1, 0)),
        ("ra", &[""], (0, 0), &[""], (0, 0)),
        // `R`: overwrite, append past the end, `<BS>` restores, a count replays.
        ("Rxy<Esc>", &["abc"], (0, 0), &["xyc"], (0, 1)),
        ("Rxyzw<Esc>", &["abc"], (0, 0), &["xyzw"], (0, 3)),
        ("Rxy<BS><Esc>", &["abc"], (0, 0), &["xbc"], (0, 0)),
        ("Rxyzw<BS><BS><Esc>", &["abc"], (0, 0), &["xyc"], (0, 1)),
        ("2Rab<Esc>", &["abcdefgh"], (0, 0), &["ababefgh"], (0, 3)),
        ("2Rab<Esc>", &["abc"], (0, 1), &["aabab"], (0, 4)),
        ("Rx<CR>y<Esc>", &["abcd"], (0, 1), &["ax", "yd"], (1, 0)),
        ("Rx<CR><BS><Esc>", &["abcd"], (0, 1), &["axcd"], (0, 1)),
        ("R<BS><BS>x<Esc>", &["abcd"], (0, 2), &["xbcd"], (0, 0)),
        // A move restarts the session: `<BS>` restores nothing behind it, a
        // count no longer replays.
        ("Rxy<Left><BS><Esc>", &["abcd"], (0, 0), &["xycd"], (0, 0)),
        ("Rxy<Left>z<Esc>", &["abcd"], (0, 0), &["xzcd"], (0, 1)),
        (
            "2Rab<Left>c<Esc>",
            &["abcdefgh"],
            (0, 0),
            &["accdefgh"],
            (0, 1),
        ),
        // …even a move back to where it was: vim restarts on the arrow key.
        (
            "Rxy<Left><Right><BS><Esc>",
            &["abcdef"],
            (0, 0),
            &["xycdef"],
            (0, 0),
        ),
        (
            "2Rxy<Left><Right><Esc>",
            &["abcdefgh"],
            (0, 0),
            &["xycdefgh"],
            (0, 1),
        ),
        // Visual `~`/`u`/`U`/`r`: in place, cursor to the selection's start.
        ("vl~", &["abcd"], (0, 1), &["aBCd"], (0, 1)),
        ("vlu", &["ABCD"], (0, 1), &["AbcD"], (0, 1)),
        ("vlU", &["abcd"], (0, 1), &["aBCd"], (0, 1)),
        ("vlrX", &["abcd"], (0, 1), &["aXXd"], (0, 1)),
        (
            "Vj~",
            &["ab", "cd", "ef"],
            (0, 1),
            &["AB", "CD", "ef"],
            (0, 0),
        ),
        ("vk~", &["ab", "cd"], (1, 1), &["aB", "CD"], (0, 1)),
        (
            "<C-v>jlrX",
            &["abc", "def"],
            (0, 0),
            &["XXc", "XXf"],
            (0, 0),
        ),
        ("vjr-", &["abc", "def"], (0, 1), &["a--", "--f"], (0, 1)),
    ];
    for &(keys, lines, (row, col), want, cursor) in cases {
        let mut app = normal_at(lines, row, col);
        type_keys(&mut app, keys);
        let got: Vec<String> = app.prompt().lines().collect();
        assert_eq!(got, want, "{keys:?} on {lines:?} at {:?}", (row, col));
        assert_eq!(
            app.prompt().cursor(),
            cursor,
            "{keys:?} on {lines:?}: cursor"
        );
        assert_eq!(
            app.prompt_keymap.mode(),
            Mode::Normal,
            "{keys:?} ends in Normal"
        );
    }
}

/// `R` says it is in Replace mode, and each of these is ONE undo — a whole
/// `R` session included, `<BS>` and all — as in vim. And Visual `r<CR>`,
/// where vim and nvim fill the selection with literal carriage returns
/// (`^M`), cancels instead (Steven, 10-02: fine not to copy). An earlier
/// oracle run read that as a delete — the CRs, printed raw, returned the
/// terminal's cursor and hid themselves; `strtrans` shows them.
#[test]
fn replace_family_undo_and_the_one_deviation() {
    let mut app = normal_at(&["abc"], 0, 0);
    type_keys(&mut app, "R");
    assert_eq!(app.prompt_keymap.mode(), Mode::Replace);
    assert_eq!(Mode::Replace.label(), "REPLACE");
    type_keys(&mut app, "xy<BS>z<Esc>");
    assert_eq!(app.prompt().lines().collect::<Vec<_>>(), ["xzc"]);

    for keys in ["Rxy<BS>z<Esc>", "3~", "2rq", "2r<CR>", "vl~", "vlrX", "VU"] {
        let mut app = normal_at(&["abcd"], 0, 0);
        type_keys(&mut app, keys);
        assert_ne!(app.prompt().text(), "abcd", "premise: {keys:?} changed it");
        type_keys(&mut app, "u");
        assert_eq!(app.prompt().text(), "abcd", "{keys:?} is one undo");
    }

    let mut app = normal_at(&["abcd"], 0, 1);
    type_keys(&mut app, "vlr<CR>");
    assert_eq!(app.prompt().text(), "abcd", "cancelled, not deleted");
    assert!(app.prompt_keymap.mode().is_visual(), "still selecting");
    assert!(
        app.notice().is_some_and(|n| n.contains("r<Enter>")),
        "and says why: {:?}",
        app.notice()
    );
}

/// **`tui.arrows = false` turns the arrows off, and says so** (Steven, 11-09:
/// the tutor's "You can do the same with the arrow keys") — `h` `j` `k` `l`
/// still move. The `:` line keeps them, where they recall and move and no
/// other key does; and a screen covering the frame, which has no row to say
/// why on.
#[test]
fn arrows_off_say_why_and_leave_hjkl() {
    let mut app = normal_at(&["abc"], 0, 0);
    type_keys(&mut app, "<Right>");
    assert_eq!(app.prompt().cursor(), (0, 1), "premise: on by default");
    let mut config = (*app.config).clone();
    config.tui.arrows = false;
    let _ = app.adopt_config(config);
    type_keys(&mut app, "<Right>");
    assert_eq!(app.prompt().cursor(), (0, 1), "the arrow did nothing");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("`tui.arrows = false`")),
        "and says why: {:?}",
        app.notice()
    );
    type_keys(&mut app, "l");
    assert_eq!(app.prompt().cursor(), (0, 2), "`l` moves");
    submit_command(&mut app, "new");
    type_keys(&mut app, ":<Up>");
    assert_eq!(app.cmdline.line(), Some("new"), "the `:` line recalls");
    type_keys(&mut app, "<Esc>");
    app.open_setup();
    let down = Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(app.arrive(down).is_some(), "a covering screen keeps them");
}

/// **The test seam ends as production does — with the cursor clamp.** Without
/// it `$` on `abc` rested at (0,3), past the last char, a state production
/// never shows (Reviewers A and C, round 19).
#[test]
fn the_test_seam_clamps_the_cursor_as_production_does() {
    let mut app = normal_at(&["abc"], 0, 0);
    app.key_for_test(
        KeyEvent::new(KeyCode::Char('$'), KeyModifiers::NONE),
        (80, 20),
    );
    assert_eq!(app.prompt().cursor(), (0, 2));
}

/// **A case or replace command that ran is an undo step even when it changed
/// nothing; one that failed is not** — each row nvim's answer with an undo
/// break between commands (as typed keys have): `x`, then the gesture, then
/// `u`. A step means `u` undoes the gesture and the `x` stays; no step means
/// `u` undoes the `x` (Reviewer C, round 19 — the "none when nothing changed"
/// rule these replaced was a deviation no test noticed).
#[test]
fn a_no_op_case_or_replace_is_still_an_undo_step_as_in_nvim() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        ("x~u", &["a1b"], &["1b"]),
        ("xrbu", &["abc"], &["bc"]),
        ("xvuu", &["abc"], &["bc"]),
        ("xVUu", &["aBC"], &["BC"]),
        // Failed: no step, so `u` undoes the `x`.
        ("x5rau", &["abc"], &["abc"]),
        ("xR<Esc>u", &["abc"], &["abc"]),
        ("xj~u", &["a", ""], &["a", ""]),
    ];
    for &(keys, lines, want) in cases {
        let mut app = normal_at(lines, 0, 0);
        type_keys(&mut app, keys);
        assert_eq!(
            app.prompt().lines().collect::<Vec<_>>(),
            want,
            "{keys:?} on {lines:?}"
        );
    }
}

#[test]
fn start_mode_config_sets_initial_prompt_mode() {
    let mut cfg = Config::default();
    cfg.tui.prompt.start_mode = StartMode::Insert;
    let app = test_app_with(cfg);
    // Focus starts on the prompt, whose keymap honors start_mode.
    assert_eq!(app.focused_mode(), Mode::Insert);
    // The chat pane is always Normal-only — asserted, where it was only
    // evaluated (review stop 12, T1 11).
    assert_eq!(app.chat_keymap.mode(), Mode::Normal);
}

#[tokio::test]
async fn submit_prompt_starts_turn_with_buffer_text() {
    let mut app = test_app();
    type_into(&mut app, "hi");
    app.submit_prompt();
    assert!(app.streaming(), "a turn started");
    let msgs = app.conversation().messages();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "hi");
    // Buffer was taken (cleared).
    assert!(app.prompt_mut().take().is_empty());
}

#[tokio::test]
async fn submit_prompt_ignores_empty_buffer() {
    let mut app = test_app();
    app.submit_prompt();
    assert!(!app.streaming(), "no turn for an empty prompt");
    assert!(app.conversation().messages().is_empty());

    // Whitespace-only is also ignored.
    type_into(&mut app, "   ");
    app.submit_prompt();
    assert!(!app.streaming());
}

/// **`<C-[>` leaves Insert mode, as `<Esc>` does** — sent as a terminal
/// speaking the kitty protocol sends it, Ctrl + `[` (`next_key`'s `<C-[>`),
/// not the legacy ESC byte. Then `x` deletes in Normal mode; had the key
/// done nothing, `x` was typed.
#[test]
fn ctrl_open_bracket_leaves_insert_mode_as_esc_does() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    type_keys(&mut app, "ihello<C-[>x");
    assert_eq!(app.prompt_keymap.mode(), Mode::Normal);
    assert_eq!(app.prompt().text(), "hell");
}
