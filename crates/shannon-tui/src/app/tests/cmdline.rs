//! The command line: typing, completion, history, and every command reaching its handler.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// **Every `:` command, typed bare, leaves a mark** — something on screen
/// changed, or it said why not, or it handed the event loop its work (an
/// editor, a quit). From the command table, so a row whose arm is empty, or
/// wired to nothing, fails here: emptying `:continue`'s and `:import`'s arms
/// passed the crate, since no test typed them (review stop 12, T1 3).
#[tokio::test]
async fn every_command_typed_leaves_a_mark() {
    use crate::command::TABLE;
    let trace = |app: &App| {
        // Sorted: a set's order is no change, and read raw it made a
        // command that did nothing look as though it had (found mutating).
        let mut panes: Vec<String> = (app.workspace.pane_ids().iter())
            .map(|p| format!("{p:?}"))
            .collect();
        panes.sort();
        format!(
            "{:?} {:?} {:?} {} {} {:?} {:?} {}",
            app.notices,
            panes,
            app.workspace.focused(),
            app.cmdline.is_open(),
            app.pending_confirm.len(),
            app.conversation().episode().id,
            app.prompt().text(),
            app.setup.is_some(),
        )
    };
    // Silent by vim's own rule: `:buffer` with no N keeps the buffer it is
    // on, and says nothing (`:h :buffer`).
    let quiet_by_design = ["buffer"];
    let mut silent = Vec::new();
    for spec in TABLE.iter().filter(|s| !quiet_by_design.contains(&s.name)) {
        let mut app = test_app();
        app.new_moment();
        let before = trace(&app);
        let effect = app.dispatch_command(spec.name);
        let handed_on = !matches!(effect, CmdEffect::Done(Flow::Stay));
        if !handed_on && trace(&app) == before {
            silent.push(spec.name);
        }
    }
    assert!(
        silent.is_empty(),
        "typed bare, these did nothing seen: {silent:?}"
    );
}

/// **The commands no test typed reach their own handlers** — each by a mark
/// only its handler leaves: `:continue` says there is no turn to resume,
/// `:import` its usage, `:child` and `:squash` hand the event loop their
/// `$EDITOR`, `:keymap` (and `:km`) its page. No test dispatched any of
/// them, and emptying the first two's arms passed the crate (review stop
/// 12, T1 3, M19); the mark above cannot tell one handler from another.
#[tokio::test]
async fn the_commands_no_test_typed_reach_their_own_handlers() {
    let typed = |line: &str| {
        let mut app = test_app();
        app.new_moment();
        let effect = app.dispatch_command(line);
        (
            effect,
            app.notice().map(|n| n.to_string()).unwrap_or_default(),
        )
    };
    let (effect, said) = typed("continue");
    assert!(matches!(effect, CmdEffect::Done(Flow::Stay)), "continue");
    assert!(
        said.contains("continue: the session has no turns"),
        "{said}"
    );
    let (effect, said) = typed("import");
    assert!(matches!(effect, CmdEffect::Done(Flow::Stay)), "import");
    assert!(said.contains("`:import <id>`"), "{said}");
    assert!(matches!(typed("child").0, CmdEffect::Child(_)), "child");
    assert!(matches!(typed("squash").0, CmdEffect::Squash), "squash");
    for keymap in ["keymap", "km"] {
        let (effect, _) = typed(keymap);
        assert!(
            matches!(
                effect,
                CmdEffect::View {
                    label: ":keymap",
                    ..
                }
            ),
            "{keymap}"
        );
    }
}

/// An unknown or ambiguous `:` command refuses **visibly** — the same door
/// every other refusal walks through.
#[tokio::test]
async fn an_unknown_command_says_so() {
    let mut app = test_app();
    app.dispatch_command_for_test("nosuchverb");
    assert!(
        app.notice().is_some_and(|n| n.contains("not a command")),
        "an unknown command must say so: {:?}",
        app.notice()
    );
}

#[test]
fn command_line_input_editing() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    app.open_command_line(); // `:` opened the line
    // Typing accumulates; nothing committed yet.
    assert_eq!(app.command_input(k(KeyCode::Char('E'))), None);
    app.command_input(k(KeyCode::Char('d')));
    app.command_input(k(KeyCode::Char('i')));
    app.command_input(k(KeyCode::Char('t')));
    assert_eq!(app.cmdline.line(), Some("Edit"));
    // Backspace edits.
    app.command_input(k(KeyCode::Backspace));
    assert_eq!(app.cmdline.line(), Some("Edi"));
    // Enter commits the line and closes it.
    assert_eq!(
        app.command_input(k(KeyCode::Enter)),
        Some("Edi".to_string())
    );
    assert_eq!(app.cmdline.line(), None);
    // Esc cancels.
    app.open_command_line();
    app.command_input(k(KeyCode::Char('q')));
    assert_eq!(app.command_input(k(KeyCode::Esc)), None);
    assert_eq!(app.cmdline.line(), None);
    // Backspace past the `:` (empty line) closes it.
    app.open_command_line();
    app.command_input(k(KeyCode::Backspace));
    assert_eq!(app.cmdline.line(), None);
}

/// **The command line edits with chords, and never types them.**
///
/// `KeyCode::Char(c) => push(c)` ignored modifiers, so `<C-w>` typed a `w` and
/// `<C-u>` typed a `u` — the modifier-blind class again
/// ([[feedback_teardown_doors]]), and a vim gap, since both are real
/// command-line editing keys there (`c_CTRL-W`, `c_CTRL-U`).
#[test]
fn command_line_chords_edit_rather_than_type() {
    let plain = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    let mut app = test_app();

    app.open_command_line();
    for c in "open abc def".chars() {
        app.command_input(plain(c));
    }
    assert_eq!(app.cmdline.line(), Some("open abc def"), "premise: typed");

    app.command_input(ctrl('w'));
    assert_eq!(app.cmdline.line(), Some("open abc "), "C-w kills a word");
    app.command_input(ctrl('w'));
    assert_eq!(app.cmdline.line(), Some("open "), "…and the next");
    app.command_input(ctrl('u'));
    assert_eq!(app.cmdline.line(), Some(""), "C-u kills the line");

    // And a chord with no meaning here types nothing at all.
    for c in "ls".chars() {
        app.command_input(plain(c));
    }
    app.command_input(ctrl('z'));
    assert_eq!(
        app.cmdline.line(),
        Some("ls"),
        "an unbound chord is not text"
    );
}

#[test]
fn command_history_recalls_with_up_and_down() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    submit_command(&mut app, "new");
    submit_command(&mut app, "graph");

    // Empty line: ↑ walks all of history, newest first.
    app.open_command_line();
    app.command_input(k(KeyCode::Up));
    assert_eq!(app.cmdline.line(), Some("graph"), "↑ recalls newest");
    app.command_input(k(KeyCode::Up));
    assert_eq!(app.cmdline.line(), Some("new"), "↑ again, older");
    app.command_input(k(KeyCode::Up));
    assert_eq!(app.cmdline.line(), Some("new"), "↑ at oldest stays");

    // ↓ walks back toward the (empty) draft.
    app.command_input(k(KeyCode::Down));
    assert_eq!(app.cmdline.line(), Some("graph"));
    app.command_input(k(KeyCode::Down));
    assert_eq!(
        app.cmdline.line(),
        Some(""),
        "↓ past the newest restores the empty draft"
    );
}

#[test]
fn command_history_prefix_filters_recall() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    submit_command(&mut app, "new");
    submit_command(&mut app, "graph");
    submit_command(&mut app, "grok");

    // Type "gr", then ↑ walks only entries starting with "gr" (skips "new").
    app.open_command_line();
    app.command_input(k(KeyCode::Char('g')));
    app.command_input(k(KeyCode::Char('r')));
    app.command_input(k(KeyCode::Up));
    assert_eq!(app.cmdline.line(), Some("grok"), "newest gr* match");
    app.command_input(k(KeyCode::Up));
    assert_eq!(
        app.cmdline.line(),
        Some("graph"),
        "older gr* match, skips new"
    );
    app.command_input(k(KeyCode::Up));
    assert_eq!(app.cmdline.line(), Some("graph"), "no older gr* match");

    // ↓ back, then past the newest match restores the "gr" prefix.
    app.command_input(k(KeyCode::Down));
    assert_eq!(app.cmdline.line(), Some("grok"));
    app.command_input(k(KeyCode::Down));
    assert_eq!(
        app.cmdline.line(),
        Some("gr"),
        "↓ past newest restores the prefix"
    );
}

#[test]
fn tab_completes_a_unique_command_in_place() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    type_command(&mut app, "conf"); // only `config` starts with `conf` (continue is `cont…`)
    app.command_input(k(KeyCode::Tab));
    assert_eq!(app.cmdline.line(), Some("config"));
    // A unique match doesn't open a cycling menu.
    assert!(app.completion_menu().is_none());
}

#[test]
fn tab_opens_a_wildmenu_and_cycles_both_ways() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    type_command(&mut app, ""); // bare `:` — every command is a candidate

    // Asserted against the candidate LIST, not against particular command
    // names: this test is about cycling, and pinning "the second command is
    // `Edit`" made adding a verb fail a test that has nothing to do with it.
    app.command_input(k(KeyCode::Tab));
    let (cands, sel) = app.completion_menu().expect("menu is open");
    let cands: Vec<String> = cands.to_vec();
    assert_eq!(cands.len(), crate::command::Command::ALL.len());
    assert_eq!(sel, 0);
    assert_eq!(app.cmdline.line(), Some(cands[0].as_str()));

    app.command_input(k(KeyCode::Tab));
    assert_eq!(app.completion_menu().unwrap().1, 1, "Tab → next");
    assert_eq!(app.cmdline.line(), Some(cands[1].as_str()));

    app.command_input(k(KeyCode::BackTab));
    assert_eq!(app.completion_menu().unwrap().1, 0, "S-Tab → previous");
    assert_eq!(app.cmdline.line(), Some(cands[0].as_str()));

    app.command_input(k(KeyCode::BackTab));
    assert_eq!(
        app.cmdline.line(),
        Some(cands[cands.len() - 1].as_str()),
        "S-Tab wraps to last"
    );
}

#[test]
fn tab_completes_arguments_after_the_verb() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    type_command(&mut app, "config ");
    app.command_input(k(KeyCode::Tab));
    assert_eq!(app.cmdline.line(), Some("config dump"));
    app.command_input(k(KeyCode::Tab));
    assert_eq!(app.cmdline.line(), Some("config edit"));
}

#[test]
fn editing_clears_the_completion_menu() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    type_command(&mut app, "");
    app.command_input(k(KeyCode::Tab));
    assert!(app.completion_menu().is_some(), "menu open after Tab");
    app.command_input(k(KeyCode::Char('x'))); // any edit invalidates it
    assert!(app.completion_menu().is_none(), "edit clears the menu");
}

#[test]
fn tab_on_multibyte_whitespace_does_not_panic() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    // U+00A0 (Option+Space on macOS) in the `:` line then Tab used to slice
    // mid-codepoint and crash the whole TUI. Must be a harmless no-op.
    type_command(&mut app, "open\u{a0}1");
    app.command_input(k(KeyCode::Tab));
    assert!(app.cmdline.is_open(), "line survives; no panic");
}

#[test]
fn id_completion_follows_per_command_policy() {
    use crate::command::IdSpace;
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3); // one session, 3 turns
    let episode = app.conversation().episode().id;
    let turn0 = app.conversation().episode().own_turns()[0];

    // `:open` (Nodes) completes episodes AND turns; ids are short (≥ 8 chars).
    let open = app.matching_short_ids(IdSpace::Nodes, "");
    assert!(open.iter().all(|s| s.len() >= 8), "short ids are ≥ 8 chars");
    assert!(
        open.iter().any(|s| episode.to_hex().starts_with(s)),
        "episode id offered to :open"
    );
    assert!(
        open.iter().any(|s| turn0.to_hex().starts_with(s)),
        "turn id offered to :open"
    );

    // `:delete` (Episodes) is session-scoped: episodes only, no turns.
    let del = app.matching_short_ids(IdSpace::Episodes, "");
    assert_eq!(del.len(), 1, "one session ⇒ one episode short");
    assert!(episode.to_hex().starts_with(&del[0]));

    // A prefix filters, and the empty id space gets nothing.
    let pfx = &episode.to_hex()[..4];
    let filtered = app.matching_short_ids(IdSpace::Nodes, pfx);
    assert!(filtered.iter().all(|s| s.starts_with(pfx)));
    assert!(app.matching_short_ids(IdSpace::None, "").is_empty());
}

#[test]
fn tab_completes_an_open_hash_from_the_graph() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let episode = app.conversation().episode().id;
    let turns = app.conversation().episode().own_turns().to_vec();

    // `:open <1-char prefix>` then Tab fills a real short-id from the graph.
    type_command(&mut app, &format!("open {}", &episode.to_hex()[..1]));
    app.command_input(k(KeyCode::Tab));
    let arg = app
        .cmdline
        .line()
        .and_then(|l| l.strip_prefix("open "))
        .expect("line stays `:open …`")
        .to_string();
    assert!(
        arg.len() >= 8,
        "completed to a short id, not the 1-char prefix"
    );
    let is_real = std::iter::once(episode.to_hex())
        .chain(turns.iter().map(|t| t.to_hex()))
        .any(|h| h.starts_with(&arg));
    assert!(
        is_real,
        "completed `{arg}` is a real episode/turn id prefix"
    );
}

#[test]
fn id_completion_snapshot_loads_once_and_resets_on_reopen() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    assert!(
        !app.cmdline.snapshot_loaded(),
        "no snapshot before a line opens"
    );

    type_command(&mut app, "open ");
    app.command_input(k(KeyCode::Tab)); // first id <Tab> loads the snapshot
    assert!(
        app.cmdline.snapshot_loaded(),
        "id completion caches the graph for the open line"
    );

    app.open_command_line(); // reopening drops it → fresh load next time
    assert!(
        !app.cmdline.snapshot_loaded(),
        "a fresh `:` line resets the snapshot"
    );
}

#[test]
fn command_history_skips_empties_and_consecutive_dupes() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    submit_command(&mut app, ""); // empty: not recorded
    submit_command(&mut app, "new");
    submit_command(&mut app, "new"); // consecutive dupe: not recorded

    app.open_command_line();
    app.command_input(k(KeyCode::Up));
    assert_eq!(app.cmdline.line(), Some("new"));
    app.command_input(k(KeyCode::Up));
    assert_eq!(
        app.cmdline.line(),
        Some("new"),
        "only one entry — no empty or dupe behind it"
    );
}
