//! **Folds in a transcript** (N5a, `docs/DesignDocs/navigation.md` §6) —
//! the `z` verbs on a window's own folds, what opens one for you, and what
//! is said where there is none: through the routes a keypress takes.

use super::position_marks::{SIZE, line_reading};
use super::thinking::think;
use super::*;

/// A turn that thought three lines, then answered, its transcript focused
/// and synced — the pane, and the line its thought starts on.
fn thought_turn() -> (App, crate::wm::PaneId, usize) {
    let mut app = test_app();
    app.submit("look");
    think(&mut app, &["Weighing the options\nfirst this\nthen that"]);
    app.feed_stream(StreamMsg::Chunk("The answer.".into()));
    app.feed_stream(StreamMsg::Complete);
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    let at = line_reading(&app, pane, "Weighing the options");
    (app, pane, at)
}

/// What `app` draws, in the frame its keys were typed in.
fn drawn(app: &mut App) -> String {
    rendered_text(&frame_of(app, SIZE.0, SIZE.1))
}

/// `keys` typed into that frame, each at the focused pane's own size — not
/// the frame's: typed at 80×24 to a pane drawn 78×9, a key read a window
/// the pane never had (review stop 15, A L10 / C M3).
fn type_in_pane(app: &mut App, keys: &str) {
    type_keys_in(app, Rect::new(0, 0, SIZE.0, SIZE.1), keys);
}

/// **`zo` opens the fold at the cursor, `zc` shuts it, `za` flips it** —
/// the thought drawn whole and back to its one row; the cursor keeps its
/// line throughout, as vim's does.
#[tokio::test]
async fn zo_opens_the_fold_and_zc_shuts_it() {
    let (mut app, pane, at) = thought_turn();
    assert!(!drawn(&mut app).contains("first this"), "premise: shut");
    type_in_pane(&mut app, &format!("{}Gzo", at + 1));
    assert!(drawn(&mut app).contains("first this"), "zo: open");
    type_in_pane(&mut app, "jzc");
    let shut = drawn(&mut app);
    assert!(!shut.contains("first this"), "zc from inside: shut\n{shut}");
    assert_eq!(
        app.pane_chat(pane).cursor().0,
        at + 1,
        "the cursor's line kept"
    );
    type_in_pane(&mut app, "za");
    assert!(drawn(&mut app).contains("first this"), "za: open");
    type_in_pane(&mut app, "za");
    assert!(!drawn(&mut app).contains("first this"), "za: shut");
}

/// **`zR` opens every fold, `zM` shuts every one** — a reply that fits a
/// row has nothing to shut, and stays as it is.
#[tokio::test]
async fn zr_opens_every_fold_and_zm_shuts_every_one() {
    let (mut app, _, _) = thought_turn();
    type_in_pane(&mut app, "zR");
    let open = drawn(&mut app);
    assert!(
        open.contains("first this") && open.contains("The answer."),
        "{open}"
    );
    type_in_pane(&mut app, "zM");
    let shut = drawn(&mut app);
    assert!(!shut.contains("first this"), "{shut}");
    assert!(shut.contains("The answer."), "one row: nothing to shut");
}

/// A frame tall enough to draw a thought, a record and a reply open at once.
const TALL: (u16, u16) = (80, 60);

/// A turn that thought three lines, ran a command, then answered three —
/// every kind of block that folds, each long enough to; its transcript
/// focused and synced.
fn every_kind_turn() -> App {
    let mut app = test_app();
    app.submit("look");
    think(&mut app, &["Weighing the options\nfirst this\nthen that"]);
    let mut record = crate::tool_record::Record::command("ran", "ls", None);
    record.output("a.txt\nb.txt\nc.txt");
    app.output_mut().push_tool_record(record);
    app.feed_stream(StreamMsg::Chunk(
        "The answer.\nsecond line\nthird line".into(),
    ));
    app.feed_stream(StreamMsg::Complete);
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    app
}

/// Which of [`every_kind_turn`]'s blocks are drawn open, in a [`TALL`]
/// frame: (the thought, the record, the reply).
fn open_kinds(app: &mut App) -> (bool, bool, bool) {
    let drawn = rendered_text(&frame_of(app, TALL.0, TALL.1));
    let open = |hidden_when_shut: &str| drawn.contains(hidden_when_shut);
    (open("first this"), open("b.txt"), open("second line"))
}

/// **A `:set` of one kind's rule puts that kind's folds back, and no
/// other's** (Steven, 11-01: after `zM`, `:set tui.fold.thinking=open`
/// opened every reply too) — and a `:set` of a rule to the value it has puts
/// its kind back too, as vim's `:set foldlevel` set again does: after `zR`,
/// `:set tui.fold.thinking=shut` shuts the thoughts, where it echoed and
/// did nothing.
#[tokio::test]
async fn a_set_of_one_kinds_rule_puts_that_kind_back_alone() {
    let tall = Rect::new(0, 0, TALL.0, TALL.1);
    let mut app = every_kind_turn();
    type_keys_in(&mut app, tall, "zM");
    assert_eq!(open_kinds(&mut app), (false, false, true), "premise: zM");
    app.dispatch_command_for_test("set tui.fold.thinking=open");
    let after = open_kinds(&mut app);
    assert_eq!(after, (true, false, true), "thinking's rule; zM's the rest");

    let mut app = every_kind_turn();
    type_keys_in(&mut app, tall, "zR");
    assert_eq!(open_kinds(&mut app), (true, true, true), "premise: zR");
    app.dispatch_command_for_test("set tui.fold.thinking=shut");
    let after = open_kinds(&mut app);
    assert_eq!(after, (false, true, true), "set again: thinking's rule");
}

/// **`zM` shuts the folds there are, and a reply has none until you make
/// one** (Steven, 11-03: "`zM` should fold the folds we set … Default should
/// be that we manually fold that text, but not that there is a fold there
/// already") — `zM` leaves a reply drawn; `zo` on it says there is no fold
/// and makes none; `zc` makes its fold, which `zR` opens and `zM` shuts
/// again; and `[tui.fold] reply = "open"` gives every reply one.
#[tokio::test]
async fn zm_shuts_the_folds_there_are_and_a_reply_has_none_until_made() {
    let tall = Rect::new(0, 0, TALL.0, TALL.1);
    let mut app = every_kind_turn();
    let pane = app.workspace.focused();
    type_keys_in(&mut app, tall, "zM");
    let shut = open_kinds(&mut app);
    assert_eq!(shut, (false, false, true), "zM: the reply has no fold");
    let reply = line_reading(&app, pane, "The answer.");
    app.notices.clear();
    type_keys_in(&mut app, tall, &format!("{}Gzo", reply + 1));
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("no fold here"), "{said}");
    type_keys_in(&mut app, tall, "zRzM");
    let after = open_kinds(&mut app);
    assert_eq!(after, (false, false, true), "zo made none: zM leaves it");
    type_keys_in(&mut app, tall, &format!("{}Gzc", reply + 1));
    let made = open_kinds(&mut app);
    assert_eq!(made, (false, false, false), "zc made its fold, shut");
    type_keys_in(&mut app, tall, "zR");
    assert_eq!(open_kinds(&mut app), (true, true, true), "zR opens it");
    type_keys_in(&mut app, tall, "zM");
    let again = open_kinds(&mut app);
    assert_eq!(again, (false, false, false), "zM shuts the fold made");

    let mut app = every_kind_turn();
    app.dispatch_command_for_test("set tui.fold.reply=open");
    let ruled = open_kinds(&mut app);
    assert_eq!(ruled, (false, false, true), "open: starts drawn");
    type_keys_in(&mut app, tall, "zM");
    let shut = open_kinds(&mut app);
    assert_eq!(shut, (false, false, false), "a fold by rule: zM shuts it");
}

/// **A rule changed by a file puts its kind back, and no other** — through
/// the door every change walks (`adopt_config`: `:source`, `:reload`), not
/// `:set`'s alone.
#[tokio::test]
async fn a_rule_changed_by_a_file_puts_its_kind_back_alone() {
    use shannon_core::config::{FoldStart, ShutOver};
    let mut app = every_kind_turn();
    type_keys_in(&mut app, Rect::new(0, 0, TALL.0, TALL.1), "zR");
    let mut config = (*app.config).clone();
    config.tui.fold.thinking = FoldStart::Over(ShutOver { shut_over: 1 });
    app.adopt_config(config);
    let after = open_kinds(&mut app);
    assert_eq!(after, (false, true, true), "thinking's rule; zR's the rest");
}

/// **A `:set tui.fold` puts its kind's folds back to its rule** (review
/// stop 15, E L5) — `zM` outranked every later `:set` for the run, and a
/// fold shut by hand outranks one too; vim's `:set foldlevel` re-applies
/// the level to every fold, hand-set ones included (measured, Neovim 0.12).
#[tokio::test]
async fn a_set_tui_fold_puts_its_kinds_folds_back_to_its_rule() {
    let (mut app, pane, at) = thought_turn();
    type_in_pane(&mut app, "zM");
    app.dispatch_command_for_test("set tui.fold.thinking=open");
    assert!(drawn(&mut app).contains("first this"), "the rule, after zM");
    type_in_pane(&mut app, &format!("{}Gzc", at + 1));
    assert!(
        !drawn(&mut app).contains("first this"),
        "premise: shut by hand"
    );
    app.dispatch_command_for_test("set tui.fold.thinking={shut_over=10}");
    assert!(drawn(&mut app).contains("first this"), "the rule, after zc");
    // And the keys' copy with it: `j` steps through the open thought.
    app.sync_chat();
    type_in_pane(&mut app, &format!("{}Gj", at + 1));
    assert_eq!(
        app.pane_chat(pane).cursor().0,
        at + 1,
        "into it, line by line"
    );

    // A window that set the session aside, its `zM` kept with its place,
    // comes back to the rule too.
    let (mut app, pane, _) = thought_turn();
    type_in_pane(&mut app, "zM");
    let shown = app.pane_session(pane).conversation.episode().id;
    app.dispatch_command_for_test("new");
    assert_ne!(
        app.pane_session(pane).conversation.episode().id,
        shown,
        "premise: set aside"
    );
    app.dispatch_command_for_test("set tui.fold.thinking=open");
    app.dispatch_command_for_test("b#");
    assert_eq!(
        app.pane_session(pane).conversation.episode().id,
        shown,
        "premise: back"
    );
    assert!(
        drawn(&mut app).contains("first this"),
        "the rule, there too"
    );
}

/// **Where there is no fold, `zc` says so** (vim's E490) — on a line that
/// fits a row, on a fold's separator; and a pane that folds nothing says
/// so too.
#[tokio::test]
async fn no_fold_at_the_cursor_says_so() {
    let (mut app, pane, at) = thought_turn();
    let answer = line_reading(&app, pane, "The answer.");
    for line in [answer, at + 3] {
        app.notices.clear();
        type_in_pane(&mut app, &format!("{}Gzc", line + 1));
        let said = app.notice().unwrap_or_default();
        assert!(said.contains("no fold here"), "line {line}: {said}");
    }
    app.set_focus(PaneKind::Prompt);
    app.notices.clear();
    type_in_pane(&mut app, "<Esc>zo");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("nothing folds here"), "{said}");
}

/// **What lands in a shut fold by a search or a motion along the line opens
/// it; `j` `k` `G` do not** (vim's 'foldopen' default, measured in Neovim).
#[tokio::test]
async fn a_search_or_a_motion_along_the_line_opens_a_fold() {
    let (mut app, _, at) = thought_turn();
    type_in_pane(&mut app, &format!("{at}Gjj"));
    assert!(!drawn(&mut app).contains("first this"), "j: still shut");
    type_in_pane(&mut app, &format!("{}G", at + 2));
    assert!(
        !drawn(&mut app).contains("first this"),
        "{{n}}G: still shut"
    );
    type_in_pane(&mut app, "l");
    assert!(drawn(&mut app).contains("first this"), "l: opened");
    type_in_pane(&mut app, "zMgg/then<CR>");
    assert!(drawn(&mut app).contains("first this"), "a search: opened");
}

/// **`N%` into a shut fold opens it; `ge` into one does not** — 'foldopen'
/// "percent" covers `N%` as it covers `%` (`nv_percent`), and `ge` steps
/// onto a fold as one line: both measured in Neovim at review stop 16 (A
/// H2), whose classes this round first filed the other way round.
#[tokio::test]
async fn n_percent_opens_a_fold_and_ge_does_not() {
    let (mut app, pane, at) = thought_turn();
    let lines = app.pane_chat(pane).line_count();
    let n = (1..=100)
        .find(|n: &usize| (n * lines).div_ceil(100) == at + 2)
        .expect("premise: a percent lands inside the thought");
    type_in_pane(&mut app, &format!("gg{n}%"));
    assert_eq!(
        app.pane_chat(pane).cursor().0,
        at + 1,
        "premise: landed in it"
    );
    assert!(drawn(&mut app).contains("first this"), "N%: opened");

    let (mut app, pane, at) = thought_turn();
    let after = (at + 3..app.pane_chat(pane).line_count())
        .find(|&l| app.pane_chat(pane).line_text(l).as_deref() == Some(""))
        .expect("premise: a blank line after the thought");
    type_in_pane(&mut app, &format!("{}Gge", after + 1));
    assert_eq!(
        app.pane_chat(pane).cursor().0,
        at + 2,
        "premise: `ge` landed in the thought"
    );
    assert!(!drawn(&mut app).contains("first this"), "ge: still shut");
}

/// **`}` from inside a shut fold steps past it whole** — to the first empty
/// line after it, the fold left shut, as vim's `findpar` skips the fold it
/// starts in. It stopped on a blank line the fold hid, and opened it
/// (review stop 16, A M4, measured in Neovim).
#[tokio::test]
async fn a_paragraph_from_a_shut_fold_steps_past_it() {
    let mut app = test_app();
    app.submit("look");
    think(&mut app, &["Weighing the options\n\nfirst this\nthen that"]);
    app.feed_stream(StreamMsg::Chunk("The answer.".into()));
    app.feed_stream(StreamMsg::Complete);
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    let at = line_reading(&app, pane, "Weighing the options");
    let chat = app.pane_chat(pane);
    let (_, past) = chat.fold_at(at).expect("premise: the thought shut");
    let blank = (past..chat.line_count())
        .find(|&l| chat.line_text(l).as_deref() == Some(""))
        .expect("premise: a blank line after the thought");
    type_in_pane(&mut app, &format!("{}G}}", at + 1));
    assert_eq!(app.pane_chat(pane).cursor().0, blank, "past the fold");
    assert!(
        !drawn(&mut app).contains("first this"),
        "the fold left shut"
    );
}

/// **A mark in a shut fold opens it when gone to** ('foldopen' "mark").
#[tokio::test]
async fn going_to_a_mark_in_a_shut_fold_opens_it() {
    let (mut app, pane, at) = thought_turn();
    type_in_pane(&mut app, &format!("{}Gzojma", at + 2));
    type_in_pane(&mut app, "zcgg");
    assert!(!drawn(&mut app).contains("first this"), "premise: shut");
    type_in_pane(&mut app, "'a");
    assert_eq!(app.pane_chat(pane).cursor().0, at + 2, "{:?}", app.notice());
    assert!(drawn(&mut app).contains("then that"), "opened");
}

/// **What opens a fold for you opens it to the keys too** — the next `j` or
/// `k` steps into the lines now drawn, not over them: the open was drawn at
/// once, and the copy the keys move in kept the block shut until something
/// re-synced it (review stop 15, A H2 / B H3 / C M2 — the re-sync was held
/// by nothing). Keys at the pane's own size, no frame between: a frame
/// re-syncs, and would hide it.
#[tokio::test]
async fn what_opens_a_fold_for_you_opens_it_to_the_keys() {
    let frame = Rect::new(0, 0, SIZE.0, SIZE.1);
    // (route, keys that open the fold, the key after, the line it lands on)
    let routes = |at: usize| {
        [
            (
                "a motion along the line",
                format!("{}Gl", at + 1),
                "j",
                at + 1,
            ),
            ("a search", "gg/first<CR>".to_string(), "j", at + 2),
            ("a mark", format!("{}Gzojmazcgg'a", at + 2), "k", at + 1),
        ]
    };
    for (route, opens, then, lands) in routes(thought_turn().2) {
        let (mut app, pane, _) = thought_turn();
        type_keys_in(&mut app, frame, &opens);
        type_keys_in(&mut app, frame, then);
        assert_eq!(app.pane_chat(pane).cursor().0, lands, "{route}");
    }
}

/// **Two windows on one session fold apart** — vim's folds are the
/// window's: opening one in a split leaves the other as it was.
#[tokio::test]
async fn two_windows_on_one_session_fold_apart() {
    let (mut app, first, at) = thought_turn();
    app.dispatch_command_for_test("split");
    let second = app.workspace.focused();
    assert_ne!(second, first, "premise: a second window");
    app.sync_chat();
    type_in_pane(&mut app, &format!("{}Gzo", at + 1));
    let shut = |app: &App, pane| {
        let output = &app.pane_session(pane).output;
        output.shut_lines(&app.pane_shut(pane), None)
    };
    assert_eq!(shut(&app, second), Vec::new(), "opened here");
    assert_eq!(shut(&app, first), vec![(at, at + 3)], "and shut there");
}

/// **A split starts with the window's folds, then folds apart** — vim's
/// `:split` copies them, opened and shut (measured in Neovim 0.12.6,
/// 11-01): the thought opened here is open there, and shutting it there
/// leaves it open here.
#[tokio::test]
async fn a_split_starts_with_the_windows_folds() {
    let (mut app, first, at) = thought_turn();
    type_in_pane(&mut app, &format!("{}Gzo", at + 1));
    app.dispatch_command_for_test("split");
    let second = app.workspace.focused();
    assert_ne!(second, first, "premise: a second window");
    app.sync_chat();
    let shut = |app: &App, pane| {
        let output = &app.pane_session(pane).output;
        output.shut_lines(&app.pane_shut(pane), None)
    };
    assert_eq!(shut(&app, second), Vec::new(), "opened there, so here");
    type_in_pane(&mut app, "zc");
    assert_eq!(shut(&app, second), vec![(at, at + 3)], "shut here");
    assert_eq!(shut(&app, first), Vec::new(), "and still open there");
}

/// **A window back in its session finds its folds as it left them** —
/// vim keeps a window's folds for a buffer it shows another over (`:enew`,
/// `:b 1`, measured): `:new`, then `<C-^>` back. From the transcript's
/// first line, where the window's place is put without moving the cursor.
#[tokio::test]
async fn a_window_back_in_its_session_finds_its_folds() {
    let (mut app, pane, at) = thought_turn();
    type_in_pane(&mut app, &format!("{}Gzogg", at + 1));
    assert_eq!(
        app.pane_chat(pane).cursor(),
        (0, 0),
        "premise: the first line"
    );
    app.dispatch_command_for_test("new");
    assert!(
        !drawn(&mut app).contains("Weighing"),
        "premise: another session shown"
    );
    press_ctrl_hat(&mut app, false);
    let back = drawn(&mut app);
    assert!(back.contains("first this"), "still open\n{back}");
}

/// **`[tui.fold]` decides how a thought starts, and `:set` shows at once**
/// — `thinking=open` draws it whole on the next frame, `thinking=shut`
/// brings it back to its row; the rule read at every draw
/// (`App::fold_rule`), never copied.
#[tokio::test]
async fn set_tui_fold_decides_how_a_thought_starts() {
    let (mut app, _, _) = thought_turn();
    assert!(
        !drawn(&mut app).contains("first this"),
        "premise: shipped shut"
    );
    app.dispatch_command_for_test("set tui.fold.thinking=open");
    let open = drawn(&mut app);
    assert!(open.contains("first this"), "{:?}\n{open}", app.notice());
    app.dispatch_command_for_test("set tui.fold.thinking=shut");
    assert!(
        !drawn(&mut app).contains("first this"),
        "{:?}",
        app.notice()
    );
    // The third spelling, which `:set` refused in its own words (review
    // stop 15, B M2): a three-line thought is open under 5, shut over 2.
    app.dispatch_command_for_test("set tui.fold.thinking={shut_over=5}");
    let open = drawn(&mut app);
    assert!(open.contains("first this"), "{:?}\n{open}", app.notice());
    app.dispatch_command_for_test("set tui.fold.thinking={shut_over=2}");
    assert!(
        !drawn(&mut app).contains("first this"),
        "{:?}",
        app.notice()
    );
}

/// **`:set tui.fold` reaches the keys as it reaches the screen** — `j` from
/// a thought's first line steps into it once the rule opens it, and over it
/// once the rule shuts it again: the drawing reads the rule each frame, the
/// keys only through the copy they move in, made at a sync — and a `:set`
/// asked for none (review stop 15, B H2). A frame first, as the loop draws
/// one, so nothing streamed has left a sync owed.
#[tokio::test]
async fn set_tui_fold_reaches_the_keys_too() {
    let frame = Rect::new(0, 0, SIZE.0, SIZE.1);
    let (mut app, pane, at) = thought_turn();
    drawn(&mut app);
    app.dispatch_command_for_test("set tui.fold.thinking=open");
    drawn(&mut app);
    type_keys_in(&mut app, frame, &format!("{}Gj", at + 1));
    assert_eq!(app.pane_chat(pane).cursor().0, at + 1, "open: into it");
    app.dispatch_command_for_test("set tui.fold.thinking=shut");
    drawn(&mut app);
    type_keys_in(&mut app, frame, &format!("{}Gj", at + 1));
    let line = app.pane_chat(pane).cursor().0;
    assert!(line > at + 2, "shut: over it, at {line}");
}

/// **`H` `M` `L` count a shut fold as one line**, as vim's do (Neovim 0.12:
/// a closed fold of 31 lines in a 22-row window, `M` on the line after it):
/// counted by the lines it hides, `M` landed inside a long shut thought, on
/// a line no row draws (review stop 15, B M1). Keys at the pane's size.
#[tokio::test]
async fn h_m_l_count_a_shut_fold_as_one_line() {
    let mut app = test_app();
    app.submit("look");
    let steps: Vec<String> = (0..40).map(|i| format!("step {i}")).collect();
    think(&mut app, &[&steps.join("\n")]);
    app.feed_stream(StreamMsg::Chunk("The answer.".into()));
    app.feed_stream(StreamMsg::Complete);
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    let frame = Rect::new(0, 0, SIZE.0, SIZE.1);
    drawn(&mut app);
    for keys in ["ggM", "ggL", "gg3H", "G3L"] {
        type_keys_in(&mut app, frame, keys);
        let line = app.pane_chat(pane).cursor().0;
        let text = app.pane_chat(pane).line_text(line).unwrap_or_default();
        let hidden = text.starts_with("step ") && text != "step 0";
        assert!(!hidden, "{keys}: on {text:?}, inside the fold");
    }
}

/// **A long line drawn shut is shut to the keys too** — a one-line thought
/// wrapped in a narrow window is drawn as a fold (vim's `foldminlines` in
/// screen lines), and `w` from its row goes past it, as vim's does from a
/// closed fold, not along the line and open: the keys' copy counted lines
/// and folded none (review stop 15, A L9 / D M7).
#[tokio::test]
async fn a_long_line_drawn_shut_is_shut_to_the_keys() {
    let mut app = test_app();
    app.submit("look");
    // One row in a wide window, two in the test's: drawn wide first, so
    // the copy is synced at a width it does not fold at, and only the
    // narrower width can owe it another sync.
    let long = ["weighing it"; 9].join(" ");
    think(&mut app, &[&long]);
    app.feed_stream(StreamMsg::Chunk("The answer.".into()));
    app.feed_stream(StreamMsg::Complete);
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    let frame = Rect::new(0, 0, SIZE.0, SIZE.1);
    let wide = rendered_text(&frame_of(&mut app, 200, SIZE.1));
    assert!(!wide.contains("· 1 line"), "premise: one row wide\n{wide}");
    assert!(drawn(&mut app).contains("· 1 line"), "premise: drawn shut");
    let at = line_reading(&app, pane, &long);
    type_keys_in(&mut app, frame, &format!("{}Gw", at + 1));
    assert!(drawn(&mut app).contains("· 1 line"), "still shut");
    assert_ne!(app.pane_chat(pane).cursor().0, at, "past it");
}

/// **`yy` on a shut fold says how many lines it took** — vim's 'report':
/// more than two lines yanked are counted (Neovim 0.12: "4 lines yanked"
/// off a closed fold of four). A shut record's lines were never drawn, and
/// a `y` that took them all said nothing; one holding a value says that
/// too (review stop 15, E M4).
#[tokio::test]
async fn yy_on_a_shut_fold_says_how_many_lines_it_took() {
    use crate::clipboard::MemoryClipboard;
    let frame = Rect::new(0, 0, SIZE.0, SIZE.1);
    for held in [false, true] {
        let mut app = test_app();
        let clip = MemoryClipboard::default();
        app.osc = Box::new(clip.clone());
        if held {
            let mut table = shannon_core::Aliases::default();
            table
                .insert("demo", "demo-value-123", "test", true)
                .unwrap();
            app.aliases = Arc::new(table);
        }
        app.submit("look");
        let out = "one\ntwo\nthree demo-value-123\nfour";
        app.output_mut().push_tool_activity("⚙ output of: ls", out);
        app.feed_stream(StreamMsg::Complete);
        app.set_focus(PaneKind::Output);
        app.sync_chat();
        assert!(drawn(&mut app).contains("· "), "premise: shut");
        let pane = app.workspace.focused();
        let at = line_reading(&app, pane, "⚙ output of: ls");
        app.notices.clear();
        type_keys_in(&mut app, frame, &format!("{}Gyy", at + 1));
        let taken = clip.last().unwrap_or_default();
        assert!(
            taken.contains("four"),
            "held {held}: the whole record {taken:?}"
        );
        let n = taken.lines().count();
        let said = app.notice().unwrap_or_default();
        assert!(
            said.starts_with(&format!("{n} lines yanked")),
            "held {held}: {said:?}"
        );
        assert_eq!(said.contains("with the value of {{demo"), held, "{said:?}");
    }
}

/// **Two lines yanked say nothing, three say so** — vim's 'report' is 2:
/// it speaks when MORE than that many lines were taken (the close verifier
/// at review stop 15, LOW 2: `> 1` passed every test).
#[tokio::test]
async fn a_yank_says_how_many_lines_only_past_two() {
    use crate::clipboard::MemoryClipboard;
    let mut app = test_app();
    app.osc = Box::new(MemoryClipboard::default());
    app.submit("look");
    app.feed_stream(StreamMsg::Chunk("one\ntwo\nthree".into()));
    app.feed_stream(StreamMsg::Complete);
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    let at = line_reading(&app, pane, "one");
    for (count, said) in [(2, None), (3, Some("3 lines yanked"))] {
        app.notices.clear();
        type_in_pane(&mut app, &format!("{}G{count}yy", at + 1));
        assert_eq!(app.notice().as_deref(), said, "{count}yy");
    }
}

/// **A fold row cut short says so** — `…` where its first line stopped,
/// then its count, the whole row inside the pane.
#[tokio::test]
async fn a_fold_row_cut_short_says_so() {
    let mut app = test_app();
    app.submit("look");
    let long = "A first line far too long to fit on one row of a pane this narrow";
    think(&mut app, &[&format!("{long}\nsecond")]);
    app.feed_stream(StreamMsg::Complete);
    let buf = frame_of(&mut app, 40, 20);
    let drawn = rendered_text(&buf);
    let row = (drawn.lines())
        .find(|r| r.contains("+ A first"))
        .unwrap_or_else(|| panic!("the fold row:\n{drawn}"));
    assert!(row.contains("… · 2 lines"), "{row:?}");
    assert!(!drawn.contains("second"), "shut");
    // The `…` a decoration, faint as the `+` and the count are — not the
    // thought's own text (review stop 15, C L-f: its style read by none).
    let faint = app.theme().ui(crate::style::Ink::TextFaint);
    let cut = super::stop::cell_at(&buf, "… · 2 lines", 0);
    assert_eq!(cut.style().fg, faint.fg, "{:?}", cut.style());
}

/// A turn whose one round asks for `calls`, each allowed — the gate's
/// answer where it asks — and started, its `⚙ running:` line drawn; the
/// recorder answers each at once, its outcomes returned unhandled.
async fn started(app: &mut App, calls: &[(&str, &str)]) -> Vec<shannon_core::ExecOutcome> {
    app.submit("look");
    app.feed_stream(StreamMsg::Chunk("Before.".into()));
    for &(call, command) in calls {
        let use_ = shannon_core::Block::bash_call(call, command);
        app.feed_stream(StreamMsg::ToolUse(use_));
    }
    app.feed_stream(StreamMsg::Complete);
    for _ in calls {
        if app.shown_call().is_some() {
            tool_key(app, 'a');
        }
    }
    let lines = app.output().lines();
    for (_, command) in calls {
        let running = format!("⚙ running: {command}");
        assert!(
            lines.contains(&running),
            "premise: {running:?} in {lines:#?}"
        );
    }
    std::iter::from_fn(|| app.exec_rx.try_recv().ok()).collect()
}

/// **A shut record says what ran and how it ended** (review stop 15, E H2
/// and F H2) — `ran` or `failed` and the command its call ran as, where
/// every record read `+ ⚙ output of: · N lines`, a failed run's as a clean
/// one's; each in its own running line's place, whichever ended first, so
/// no `⚙ running:` stays above a command that ended.
#[tokio::test]
async fn a_shut_record_says_what_ran_and_how_it_ended() {
    let mut app = test_app();
    let calls = [
        ("tu_ok", "echo the tide"),
        ("tu_bad", "grep -q tide nothing.txt"),
    ];
    let mut ended = started(&mut app, &calls).await;
    assert_eq!(ended.len(), 2, "premise: both ran");
    ended.reverse();
    for mut outcome in ended {
        outcome.code = Some(if outcome.tool_use_id == "tu_bad" {
            2
        } else {
            0
        });
        app.handle_exec_outcome(outcome);
        app.settle();
    }
    let buf = frame_of(&mut app, SIZE.0, SIZE.1);
    let shown = rendered_text(&buf);
    let row = |text: &str| {
        (shown.lines())
            .position(|row| row.contains(text))
            .unwrap_or_else(|| panic!("{text:?} in:\n{shown}"))
    };
    let ok = row("+ ⚙ ran: echo the tide · ");
    let bad = row("+ ⚙ failed: grep -q tide nothing.txt · ");
    assert!(ok < bad, "each where it started:\n{shown}");
    assert!(!shown.contains("running"), "{shown}");
    // The title in the record's colours, as the running line wore them.
    let at = cell_of(&buf, "⚙ failed").expect("drawn");
    assert_eq!(buf[at].style().fg, Some(crate::palette::TOOL));
}

/// **A titled fold row is selected whole or not at all** — its cells are
/// no line's (vim lights a closed fold whole or not at all, `fold_line`):
/// `V` on it takes the whole record and lights the whole row; a selection
/// reaching only into its first line lights none of it. Laid on from its
/// header line, `V` lit the row's first six cells and `vk` its first.
#[tokio::test]
async fn a_titled_fold_row_is_selected_whole_or_not_at_all() {
    use ratatui::style::Modifier;
    const TITLE: &str = "⚙ ran: echo the tide";
    for (keys, whole) in [("V", true), ("vk", false)] {
        let mut app = test_app();
        for outcome in started(&mut app, &[("tu_1", "echo the tide")]).await {
            app.handle_exec_outcome(outcome);
            app.settle();
        }
        app.feed_stream(StreamMsg::Chunk("After.".into()));
        app.feed_stream(StreamMsg::Complete);
        app.set_focus(PaneKind::Output);
        app.sync_chat();
        let pane = app.workspace.focused();
        let at = line_reading(&app, pane, "⚙ ran:");
        type_in_pane(&mut app, &format!("{}G{keys}", at + 1));
        let buf = frame_of(&mut app, SIZE.0, SIZE.1);
        let at = cell_of(&buf, TITLE).unwrap_or_else(|| panic!("{}", rendered_text(&buf)));
        let lit: Vec<bool> = (at.x..at.x + TITLE.chars().count() as u16)
            .map(|x| buf[(x, at.y)].modifier.contains(Modifier::REVERSED))
            .collect();
        assert!(lit.iter().all(|&on| on == whole), "{keys}: {lit:?}");
    }
}
