//! The status row, alerts and startup notes — what the row describes.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// **A refusal beside a launch note is the part the row shows** — the row
/// is coloured by its loudest notice and cut from its end, and the notices
/// were joined as they arrived: `shannon --resume nosuch` drew the stub
/// key's note in a refusal's colour and the refusal as "`:resume`:
/// …:messages" (Steven's smoke run, 10-19). The loudest first now.
#[tokio::test]
async fn a_refusal_beside_a_launch_note_is_the_part_shown() {
    let mut app = test_app();
    app.report_startup(&[
        "key file `/dev/null` is empty — answering from the canned stub, not the API".into(),
    ]);
    app.resume_command("nosuch-session");
    let screen = text_of(&mut app, 100, 30);
    let row = screen.lines().last().unwrap_or_default();
    assert!(
        row.contains("nosuch-session") && row.contains("…:messages"),
        "the refusal, and the note cut after it: {row:?}"
    );
}

/// **A startup note reaches the screen, and `:messages`.**
///
/// The launch path is where "never only in a log" kept being broken: a rejected
/// keymap override, a config that would not parse, a history store that would
/// not open, an API key that resolved to nothing. Each producer is tested where
/// it lives; this is the CONSUMER end, and it is the half that decides whether
/// any of them is visible at all. Without it, `report_startup` could be gutted
/// and every producer test would still pass ([[feedback_wiring_is_untested]]).
///
/// `:messages` as well as the row, because the row is one line and shows only
/// the most recent moment — a launch that produced three notes must not lose
/// two of them.
#[tokio::test]
async fn a_startup_note_reaches_the_screen_and_the_message_log() {
    let mut app = test_app();
    assert!(
        app.notice().is_none(),
        "premise — a fresh app says nothing: {:?}",
        app.notice()
    );
    let notes = [
        "no API key found — answering from the canned stub".to_string(),
        "history is NOT being saved".to_string(),
        "keymap: unknown action `submitt`".to_string(),
    ];
    app.report_startup(&notes);

    let row = app
        .notice()
        .expect("startup notes must reach the status row");
    for note in &notes {
        assert!(
            row.contains(note.as_str()),
            "the row dropped {note:?}: {row}"
        );
        assert!(
            app.messages().any(|m| m == note),
            "`:messages` dropped {note:?}"
        );
    }
}

/// **`$alerts` says what the format does not place itself** — a hidden
/// decision in `$alerts` for a row with no `${alert.hidden}`, and only where
/// it is written for a row with one: the shipped rows put it at the right,
/// and `$alerts` beside the pane's name said it a second time. Its value
/// written bare is not the alert (B1.1): a file holding the 10-22 template's
/// `$hidden` says it in both places.
#[tokio::test]
async fn alerts_say_a_hidden_decision_unless_the_row_places_it() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    to_decision(&mut app);
    app.dispatch_command_for_test("hide");
    let row = status_row(&mut app, "prompt", "$alerts");
    assert!(row.contains("decision hidden"), "{row}");
    let row = status_row(&mut app, "prompt", "<$alerts>${alert.hidden}");
    assert_eq!(row.matches("decision hidden").count(), 1, "{row}");
    assert!(
        row.contains(">decision hidden"),
        "where it is written: {row}"
    );
    let row = status_row(&mut app, "prompt", "<$alerts>$hidden");
    assert_eq!(row.matches("decision hidden").count(), 2, "{row}");
}

/// **`:set` reaches a shipped alert, and the row takes it at once** — its
/// `say` changed through the door every config change walks
/// (`load_theme`), and `disabled` added to: the alerts are read again with
/// the formats, not once at launch.
#[tokio::test]
async fn a_set_changes_an_alert_on_the_next_frame() {
    let mut app = test_app();
    // Queued behind a turn that runs, as a prompt waits in the loop: an idle
    // session's starts at the event's end, which the `:set` now is.
    app.fake_stream_on(app.current);
    app.pending.push_back((app.current, "later".into()));
    // The running turn's own alert stands beside it, unchanged throughout.
    let row = status_row(&mut app, "prompt", "<$alerts>");
    assert!(
        row.contains("<1 queued · ^C:stop>"),
        "premise: the shipped say: {row}"
    );
    app.dispatch_command_for_test("set tui.alert.queued.say=WAITING");
    let row = status_row(&mut app, "prompt", "<$alerts>");
    assert!(row.contains("<WAITING · ^C:stop>"), "{row}");
    app.dispatch_command_for_test("set tui.alerts.disabled+=queued");
    let row = status_row(&mut app, "prompt", "<$alerts>");
    assert!(row.contains("<^C:stop>"), "{row}");
}

/// **A theme role in the row is the theme's when drawn** (B1.4) — in a
/// format and in an alert's `say`, asserted on the drawn cells; and a theme
/// changed between frames recolours them on the next, with no format read
/// again (a role is kept as the role, DR-B #12).
#[test]
fn a_role_in_the_row_is_the_themes_on_every_frame() {
    use ratatui::style::Color;
    let dir = tempfile::tempdir().unwrap();
    for (name, danger) in [("one", "#ff0000"), ("two", "#00ff00")] {
        let file = format!("\"ui.danger\" = \"{danger}\"\n\"ui.caution\" = \"{danger}\"\n");
        std::fs::write(dir.path().join(format!("{name}.toml")), file).unwrap();
    }
    let theme = |name| {
        crate::theme::Theme::load(name, Some(dir.path()))
            .expect("loads")
            .0
    };
    let cfg = Config::parse(
        "[tui.status]\nprompt = '[$mode](ui.danger) $alerts'\n\
         [tui.alert.queued]\nsay = '[WAIT](fg:ui.caution)'\n",
    )
    .unwrap();
    let mut app = test_app_with(cfg);
    app.set_focus(PaneKind::Prompt);
    app.pending.push_back((app.current, "later".into()));
    let fg_of = |app: &mut App, word: &str| {
        app.notices.clear();
        let frame = frame_of(app, 80, 20);
        let buf = frame.clone();
        let y = buf.area().height - 1;
        let row: String = (0..buf.area().width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect();
        let at = row
            .find(word)
            .unwrap_or_else(|| panic!("{word} drawn: {row}"));
        buf[(row[..at].chars().count() as u16, y)].style().fg
    };
    app.set_theme(theme("one"));
    let red = Some(Color::Rgb(0xff, 0, 0));
    assert_eq!(fg_of(&mut app, "NORMAL"), red, "a format's role");
    assert_eq!(fg_of(&mut app, "WAIT"), red, "a say's role");
    app.set_theme(theme("two"));
    let green = Some(Color::Rgb(0, 0xff, 0));
    assert_eq!(fg_of(&mut app, "NORMAL"), green, "the next frame's theme");
    assert_eq!(fg_of(&mut app, "WAIT"), green, "and the say's");
}

/// **An alert of yours holds on the facts of the session the row is about**
/// (B1.2) — `[tui.alert.*]` from a config: `ctx_percent >= 80` says nothing
/// at half full and its `say` at 85%, of the aim — a second session at 90%
/// raises nothing on the aim's row — and a `when` the config cannot read is
/// drawn in its place and said at launch.
#[test]
fn an_alert_of_yours_holds_on_the_aims_facts() {
    // The format is the config's, not a `:set`'s: a `:set` folds the config
    // again from its files, and this one has none.
    let cfg = Config::parse(
        "[tui.status]\nprompt = '<$alerts>'\n\
         [tui.alert.hot]\nwhen = \"ctx_percent >= 80\"\nsay = \"HOT $ctx\"\n\
         [tui.alert.typo]\nwhen = \"ctx >= 80\"\n",
    )
    .unwrap();
    let mut app = test_app_with(cfg);
    let fill = |app: &mut App, buffer: BufferId, pct: u64| {
        let budget = u64::from(app.context_budget(buffer));
        app.session_of_mut(buffer).unwrap().billing.context = (budget * pct / 100) as u32;
    };
    let row = |app: &mut App| {
        app.new_moment();
        screen(app, 100)
            .lines()
            .last()
            .expect("a status row")
            .to_string()
    };
    let a = app.current;
    fill(&mut app, a, 50);
    let drawn = row(&mut app);
    assert!(drawn.contains("<[typo: when?]>"), "half full: {drawn}");
    fill(&mut app, a, 85);
    let drawn = row(&mut app);
    assert!(drawn.contains("<HOT 85% · [typo: when?]>"), "{drawn}");
    // Aimed at a second session, nearly empty, while the first fills (a
    // session with no turn is the one `:new` would give again).
    app.conversation_mut().begin_turn("A 0");
    app.conversation_mut().push_chunk("r");
    app.conversation_mut().commit_turn();
    app.new_session_command();
    let b = app.current;
    assert_ne!(a, b, "premise: a second session");
    assert_eq!(app.prompt_target(), b, "premise: aimed at it");
    fill(&mut app, a, 90);
    fill(&mut app, b, 10);
    let drawn = row(&mut app);
    assert!(
        drawn.contains("<[typo: when?]>"),
        "the aim's, not the first's: {drawn}"
    );
    // Said where every config's notes are said.
    let said = Config::parse("[tui.alert.typo]\nwhen = \"ctx >= 80\"\n").unwrap();
    let (_, notes) = crate::status_line::Formats::read(&said.tui);
    assert!(
        notes
            .iter()
            .any(|n| n.contains("the fact is `ctx_percent`")),
        "{notes:?}"
    );
}

/// **A refusal and a report look different on the row.**
///
/// They differed only by `warn!` vs `info!` in the log — invisible to the
/// user, so the distinction cost nothing to get wrong and was got wrong across
/// roughly eight commands (Reviewer D counted them; `:analyze` disagreed with
/// itself seven lines apart). Steven's call: make it visible, so the split
/// earns its keep.
///
/// The rule: **refusal = you asked and we said no, nothing happened; report =
/// we did something and are telling you.** Red is reused from
/// `Decision::Deny`, not invented — one vocabulary for "no".
///
/// Asserted on the **rendered cells**, because the colour is the whole feature
/// and a `NoticeKind` returned by a getter is not a colour.
#[test]
fn a_refusal_and_a_report_are_not_the_same_colour() {
    let row_colour = |app: &mut App| -> Option<ratatui::style::Color> {
        let frame = frame_of(app, 100, 12);
        let buf = &frame;
        let y = buf.area().height - 1;
        // The first cell with ink on it — the notice starts one space in.
        (0..buf.area().width)
            .map(|x| buf[(x, y)].clone())
            .find(|cell| cell.symbol().trim() != "")
            .and_then(|cell| cell.style().fg)
    };

    let mut app = test_app();
    app.notices.clear();
    app.refuse("no, and here is why");
    let refused = row_colour(&mut app);

    let mut app2 = test_app();
    app2.notices.clear();
    app2.notify("done, and here is what happened");
    let reported = row_colour(&mut app2);

    assert!(
        refused.is_some() && reported.is_some(),
        "both reach the row"
    );
    assert_ne!(
        refused, reported,
        "a refusal must not look like a report: both rendered {refused:?}"
    );
    assert_eq!(
        refused,
        Some(ratatui::style::Color::Red),
        "red is `Decision::Deny`'s colour, reused rather than a new one"
    );

    // **The loudest wins a mixed batch**, or a report arriving alongside a
    // refusal would dress it down.
    let mut both = test_app();
    both.notices.clear();
    both.notify("something happened");
    both.refuse("and something else was declined");
    assert_eq!(
        row_colour(&mut both),
        Some(ratatui::style::Color::Red),
        "a batch is as loud as its loudest member"
    );
}

/// **A pending decision survives on the status row while you read a viewer.**
///
/// The viewer legend used to `return` from `draw_status`, taking every
/// `Essential` segment with it. So with a tool call pending and its pane too
/// short to draw, focusing `:help` left `DECIDE:` and `^C:stop` nowhere on
/// screen — while the app went on refusing new prompts, giving the user a
/// refusal whose reason was not displayed. The comment there already called
/// hiding that warning "the dangerous part" (Reviewer F).
///
/// Asserted on the rendered row: a hint table asserted directly once had all
/// five viewer arms replaced with nonsense and 1011 tests stayed green — they
/// were never drawn at all (Reviewer F).
#[tokio::test]
async fn the_status_row_keeps_essentials_under_a_viewer_legend() {
    // **The safety half, at a width where the row is tight.** A decision is
    // pending and its pane cannot be drawn; the row is the only thing that can
    // say so, and the legend must not be able to take that place.
    let mut app = test_app();
    app.arrive_and_gate_for_test(None, "t1", "rm -rf /tmp/whatever");
    app.open_help_command(None);
    app.workspace.focus_kind(PaneKind::Page(PageKind::Help));
    let frame = frame_of(&mut app, 100, 12);
    let row = rendered_text(&frame)
        .lines()
        .last()
        .expect("a status row")
        .to_string();
    assert!(
        row.contains("DECIDE"),
        "a pending decision must not vanish because you looked at the manual:\n{row}"
    );

    // **The legibility half, at an ordinary width.** The page's row says
    // what its keys cannot — where its keys and commands are listed — and
    // all of it fits at the 100x30 dogfooding found the old legend cut at.
    // That typing changes nothing here is said when you type, as vim's E21
    // is, not on the row (Steven, 10-30, S14-Q1).
    let mut app = test_app();
    app.open_help_command(None);
    app.workspace.focus_kind(PaneKind::Page(PageKind::Help));
    let row = text_of(&mut app, 100, 12)
        .lines()
        .last()
        .expect("a status row")
        .to_string();
    assert!(row.trim_end().starts_with(" [NORMAL] help · 1/"), "{row}");
    assert!(row.contains("help `:help help`"), "{row}");
    assert!(!row.contains('…'), "nothing cut at 100 columns:\n{row}");
}

/// **`ctx` sits beside the model and takes a colour as the window fills**
/// (Steven, 10-14: "move that to just right of the model name … color it
/// at given levels … 70% and 90%") — the row's own below, caution from the
/// first, danger from the second. Asserted on the drawn cells: a colour is
/// what the user sees, and a style mapped right can still be drawn wrong.
///
/// **And the stops are the user's** (`tui.ctx_colors`, Steven: "an array
/// with `percentage:#hex`"): a file's stops apply at once, through the door
/// every config change takes, and the one it cannot read is said ONCE — not
/// again at the next unrelated `:set`.
#[test]
fn ctx_is_beside_the_model_and_coloured_as_it_fills() {
    use ratatui::style::Color;
    fn drawn(app: &mut App, pct: u64) -> (String, Option<Color>) {
        // A notice takes the status row.
        app.notices.clear();
        let budget = app.context_budget(app.buffer().id);
        app.buffer_mut().billing.context = (u64::from(budget) * pct / 100) as u32;
        app.buffer_mut().billing.total.input_tokens = 1_000;
        let frame = frame_of(app, 200, 20);
        let buf = frame.clone();
        let y = buf.area().height - 1;
        let row: String = (0..buf.area().width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect();
        let at = row
            .find("ctx ")
            .unwrap_or_else(|| panic!("ctx drawn: {row}"));
        // The number takes the colour (`$ctx`); the `ctx ` before it is the
        // format's own text.
        let col = row[..at].chars().count() as u16 + 4;
        (row.clone(), buf[(col, y)].style().fg)
    }
    let mut app = test_app();
    let (row, fg) = drawn(&mut app, 50);
    let model = shannon_core::backend::models::short_name(
        app.answering_model(&app.buffer().conversation.episode().config.model),
    )
    .to_string();
    let (m, c, t) = (
        row.find(&format!(" · {model}")).expect("the model drawn"),
        row.find(" · ctx ").expect("ctx drawn"),
        row.find("tok")
            .unwrap_or_else(|| panic!("tokens drawn: {row}")),
    );
    assert!(m < c && c < t, "model, then ctx, then tokens: {row}");
    assert_eq!(fg, Some(crate::palette::TEXT), "below the warning: {row}");
    assert_eq!(
        drawn(&mut app, 75).1,
        Some(crate::palette::CAUTION),
        "from 70%"
    );
    assert_eq!(
        drawn(&mut app, 95).1,
        Some(crate::palette::DANGER),
        "from 90%"
    );

    let dir = TempDir::new().unwrap();
    let file = dir.path().join("ctx.toml");
    std::fs::write(
        &file,
        "[tui]\nctx_colors = [\"60:5\", \"40:#00ff00\", \"80:mauve\"]\n",
    )
    .unwrap();
    app.notices.clear();
    app.dispatch_command_for_test(&format!("source {}", file.display()));
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("80:mauve"),
        "the unreadable stop is said: {said}"
    );
    assert_eq!(
        drawn(&mut app, 30).1,
        Some(crate::palette::TEXT),
        "below the first"
    );
    assert_eq!(
        drawn(&mut app, 50).1,
        Some(Color::Rgb(0, 255, 0)),
        "from 40%"
    );
    assert_eq!(
        drawn(&mut app, 95).1,
        Some(Color::Indexed(5)),
        "from 60%, mauve left out"
    );
    app.notices.clear();
    app.dispatch_command_for_test("set tui.scrolloff=2");
    let said = app.notice().unwrap_or_default();
    assert!(!said.contains("mauve"), "said once: {said}");
}

/// **A status row is the user's to write, and shows at once** (Steven, 10-14:
/// "make this configurable for people") — through `:set`, the door every
/// config change takes, a space written `\ ` as vim writes one; and a value
/// the row does not have is said once and drawn as written, never dropped.
#[test]
fn set_tui_status_rewrites_the_row_and_says_what_it_cannot_read_once() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    let row = |app: &mut App| {
        app.notices.clear();
        screen(app, 120)
            .lines()
            .last()
            .unwrap_or_default()
            .to_string()
    };
    assert!(row(&mut app).starts_with(" [NORMAL] prompt"), "premise");
    app.dispatch_command_for_test(r"set tui.status.prompt=<$pane>\ $modle");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("tui.status.prompt") && said.contains("`$modle`"),
        "{said}"
    );
    assert_eq!(row(&mut app).trim_end(), " <prompt> $modle");
    app.dispatch_command_for_test("set tui.scrolloff=2");
    let said = app.notice().unwrap_or_default();
    assert!(!said.contains("modle"), "said once: {said}");
    // The other rows are their own.
    app.set_focus(PaneKind::Output);
    assert!(row(&mut app).starts_with(" [NORMAL] output"));
}

/// **`x&` puts a row back** (Steven, 10-14: `:set tui.status.prompt=?` meant
/// as a query, the row was left reading `?`). The default is vim's way back;
/// `=?` setting the value `?` is vim's too (`:set path=?`).
#[test]
fn set_tui_status_ampersand_puts_the_shipped_row_back() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    let row = |app: &mut App| {
        app.notices.clear();
        screen(app, 120)
            .lines()
            .last()
            .unwrap_or_default()
            .to_string()
    };
    let shipped = row(&mut app);
    assert!(shipped.starts_with(" [NORMAL] prompt"), "premise");
    app.dispatch_command_for_test("set tui.status.prompt=?");
    assert_eq!(row(&mut app).trim_end(), " ?", "premise: the mistake");
    app.dispatch_command_for_test("set tui.status.prompt&");
    assert_eq!(row(&mut app), shipped);
}

/// **`tui.status.tokens` reaches the row it governs** — short by default,
/// every digit after `:set tui.status.tokens=exact`, short again after `&`;
/// and a word it does not take is refused, leaving the row as it was.
#[test]
fn set_tui_status_tokens_rewrites_the_counts_on_the_row() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    app.buffer_mut().billing.total = Usage {
        input_tokens: 10_415_096,
        output_tokens: 39_996,
        ..Default::default()
    };
    let row = |app: &mut App| {
        app.notices.clear();
        screen(app, 160)
            .lines()
            .last()
            .unwrap_or_default()
            .to_string()
    };
    assert!(
        row(&mut app).contains("tok ↑10.4M ↓40k"),
        "{}",
        row(&mut app)
    );
    app.dispatch_command_for_test("set tui.status.tokens=exact");
    assert!(
        row(&mut app).contains("tok ↑10415096 ↓39996"),
        "{}",
        row(&mut app)
    );
    app.dispatch_command_for_test("set tui.status.tokens=round");
    assert!(app.notice().is_some(), "a word it does not take is said");
    assert!(row(&mut app).contains("↑10415096"), "and changes nothing");
    app.dispatch_command_for_test("set tui.status.tokens&");
    assert!(
        row(&mut app).contains("tok ↑10.4M ↓40k"),
        "{}",
        row(&mut app)
    );
}

/// **Beside the prompt, the status row's scroll marker reads the aim's
/// window** — `pane_viewport` redirects the prompt pane to it; without the
/// redirect it read the prompt bar's own, never-scrolled viewport. Deletable
/// with the suite green until now (review 2026-10-15, A F2, M17).
#[test]
fn status_scroll_by_the_prompt_reads_the_aims_window() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let t = app.workspace.focused();
    app.window_of_mut(t)
        .unwrap()
        .session_view_mut()
        .unwrap()
        .viewport
        .freeze_at(0, 0);
    assert!(
        !app.pane_viewport(t).is_following_tail(),
        "premise: scrolled back"
    );
    app.set_focus(PaneKind::Prompt);
    assert!(!app.aim_viewport().is_following_tail());
}

/// **…and when a viewer has borrowed that window, the marker still reads
/// it** (review 2026-10-16, Reviewer B F4). `:graph` over your only
/// transcript: no pane VISIBLY shows the aim, so `pane_viewport` fell back to
/// the prompt bar's own never-scrolled viewport (`↧` forever) while the
/// numbers beside it described the aim. The borrowed pane's saved window is
/// the aim's — what `host_pane` already took (`aim_window`).
#[test]
fn status_scroll_reads_the_aims_window_under_a_viewer() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let t = app.workspace.focused();
    app.window_of_mut(t)
        .unwrap()
        .session_view_mut()
        .unwrap()
        .viewport
        .freeze_at(0, 0);
    app.open_graph_command();
    assert!(
        app.window_of(t).is_some_and(Window::has_beneath),
        "premise: the graph borrowed the transcript's pane"
    );
    app.set_focus(PaneKind::Prompt);
    assert!(
        app.aim_pane().is_none(),
        "premise: nothing VISIBLY shows the aim"
    );
    assert!(
        !app.aim_viewport().is_following_tail(),
        "the marker reads the aim's window, under the viewer"
    );
}

/// **Standing in a pane that shows no session, the status row is the aimed
/// session's** — its numbers AND its tail marker (review 2026-10-15, Reviewer
/// B F3; `buffers.md` §9 call 4, Steven: "the status line is always
/// associated with the session we're aiming at").
///
/// It was the prompt's rule alone. Standing in a viewer or the decision pane,
/// `pane_session` fell back to `current` — the session in front — and the
/// viewport to the viewer's own, never-scrolled one: the row's model, context
/// and tokens described one session and its marker another pane. The aim is
/// set apart from `current` first, or the two answers agree by accident.
#[test]
fn a_pane_showing_no_session_reports_the_aimed_session() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _, _) = app_with_two_sessions(dir.path());
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    let (here, there) = (panes[0], panes[1]);
    app.focus_pane(here);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;
    app.focus_pane(there);
    app.open_by_hash(&b.to_hex()[..8]);
    app.focus_pane(here);
    assert_eq!(app.prompt_target(), a_buffer, "premise: aimed at A");
    assert_ne!(app.current, a_buffer, "premise: B is in front");
    // Scrolled back as a user does it — in a window drawn: a first draw
    // keeps a frozen window's cursor on screen (`note_transcript_sizes`).
    screen(&mut app, 100);
    app.window_of_mut(here)
        .unwrap()
        .session_view_mut()
        .unwrap()
        .viewport
        .freeze_at(0, 0);

    app.dispatch_command_for_test("sgraph");
    assert_eq!(app.focus(), PaneKind::Graph, "premise: in a viewer");
    let lines = |buffer| app.session_of(buffer).unwrap().output.line_count();
    let (a_lines, front) = (lines(a_buffer), lines(app.current));
    assert_ne!(a_lines, front, "premise: the counts tell A from B");

    let row = status_row(&mut app, "graph", r"<$lines>\ <$follow>");
    assert!(
        row.contains(&format!("<{a_lines}>")),
        "the row's numbers: {row:?}"
    );
    assert!(
        row.contains("<‖>"),
        "the row's marker reads the aim's window, not the viewer's own: {row:?}"
    );
}

/// **Standing in a lent viewer, the row is the aim's** (Steven, 10-16:
/// "The status line should always be associated with what we're aimed at").
/// The row read through `about_the_aim`, whose lent exception is the
/// pane's (`:Send <pane>`: `only_a_sessions_pane_takes_a_mark`, in
/// `aim.rs`), so a `:graph` lent over A, stood in
/// while aimed at B, described A: A's line count, and A's saved window's
/// tail marker. Drawn, with a `graph` row that shows session values — the
/// shipped one shows none.
#[test]
fn standing_in_a_lent_viewer_the_row_describes_the_aim() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _, _) = app_with_two_sessions(dir.path());
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    let (left, right) = (panes[0], panes[1]);
    app.focus_pane(left);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.prompt_target();
    app.focus_pane(right);
    app.open_by_hash(&b.to_hex()[..8]);
    let b_buffer = app.prompt_target();
    // B's window scrolled back, A's following its end: the marker tells
    // the two windows apart. Scrolled back drawn, as a user does it.
    screen(&mut app, 100);
    app.window_of_mut(right)
        .unwrap()
        .session_view_mut()
        .unwrap()
        .viewport
        .freeze_at(0, 0);
    app.focus_pane(left);
    app.dispatch_command_for_test("graph");
    app.focus_pane(right);
    app.focus_pane(left); // into the lent graph: reading, not aiming
    assert_eq!(app.focus(), PaneKind::Graph, "premise: in the graph");
    assert!(
        app.window_of(left).is_some_and(Window::has_beneath),
        "premise: lent over A's transcript"
    );
    assert_eq!(app.prompt_target(), b_buffer, "premise: aimed at B");
    let lines = |buffer| app.session_of(buffer).unwrap().output.line_count();
    let (a_lines, b_lines) = (lines(a_buffer), lines(b_buffer));
    assert_ne!(a_lines, b_lines, "premise: the counts tell A from B");
    assert!(
        app.pane_viewport(left).is_following_tail(),
        "premise: A's window follows"
    );

    let row = status_row(&mut app, "graph", r"<$lines>\ <$follow>");
    assert!(
        row.contains(&format!("<{b_lines}>")),
        "B's numbers: {row:?}"
    );
    assert!(row.contains("<‖>"), "B's window's marker: {row:?}");
}

/// **Two windows on one session, and the row's marker is the one you stand
/// in** (|window-session|: "the status line's `$follow`
/// describes the window you are standing in"). `aim_viewport` prefers the
/// focused pane when it shows the aim; `aim_window` alone is the FIRST pane
/// showing it, in ring order, which is the other window here.
#[test]
fn the_rows_marker_is_the_window_you_stand_in_on_a_shared_session() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let first = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    let second = app.workspace.focused();
    assert_ne!(first, second, "premise: two windows");
    assert_eq!(
        app.pane_session(first).id,
        app.pane_session(second).id,
        "premise: on one session"
    );
    // Whichever the ring puts first (`:vs` puts the newcomer there) is the
    // one `aim_window` names; scroll THAT one back and stand in the other.
    let named = app.aim_window().expect("a window shows the aim");
    let other = if named == first { second } else { first };
    screen(&mut app, 100); // scrolled back drawn, as a user does it
    app.window_of_mut(named)
        .unwrap()
        .session_view_mut()
        .unwrap()
        .viewport
        .freeze_at(0, 0);

    app.focus_pane(other);
    let row = status_row(&mut app, "output", r"<$follow>");
    assert!(
        row.contains("<↧>"),
        "the window you stand in follows: {row:?}"
    );
    app.focus_pane(named);
    let row = status_row(&mut app, "output", r"<$follow>");
    assert!(row.contains("<‖>"), "and this one does not: {row:?}");
}

/// **The cursor's shape says the mode as the row does** (Steven, 11-02:
/// two signals for one change), from the same keymap: a block in Normal, a
/// bar in Insert, an underline while `d` or `r` waits; the `:` line a block
/// with its cursor at the end and a bar inside it. `:set tui.guicursor=`
/// leaves the terminal's own shape; a value neovim refuses is refused on
/// screen and keeps the shapes the cursor had.
#[test]
fn the_cursor_shape_says_the_mode_as_the_row_does() {
    let mut app = test_app();
    let shape = |app: &App| app.terminal_cursor().shape;
    type_keys(&mut app, "<Esc>");
    assert_eq!(
        (app.focused_mode().label(), shape(&app)),
        ("NORMAL", Some(2))
    );
    type_keys(&mut app, "i");
    assert_eq!(
        (app.focused_mode().label(), shape(&app)),
        ("INSERT", Some(6))
    );
    type_keys(&mut app, "<Esc>d");
    assert_eq!(shape(&app), Some(4), "`d` waiting");
    type_keys(&mut app, "<Esc>r");
    assert_eq!(shape(&app), Some(4), "`r` waiting");
    type_keys(&mut app, "<Esc>:ab");
    assert!(app.cmdline.is_open());
    assert_eq!(shape(&app), Some(2), "the `:` line, at its end");
    type_keys(&mut app, "<Left>");
    assert_eq!(shape(&app), Some(6), "the `:` line, inside it");
    type_keys(&mut app, "<Esc>");
    assert_eq!(shape(&app), Some(2));

    // `c` apart from `n`: the `:` line's own, not the keymap's under it.
    app.dispatch_command_for_test("set tui.guicursor=c:hor20");
    type_keys(&mut app, ":");
    assert_eq!(shape(&app), Some(4), "`c`, not the Normal under it");
    type_keys(&mut app, "<Esc>");
    assert_eq!(shape(&app), Some(2));

    app.dispatch_command_for_test("set tui.guicursor=");
    assert_eq!(shape(&app), None, "the terminal's own");
    app.dispatch_command_for_test("set tui.guicursor=n:hor20,i:ver25");
    assert_eq!(shape(&app), Some(4), "a value with parts, set");
    app.dispatch_command_for_test("set tui.guicursor=x:block");
    // Refused as a theme that cannot be read is, beside `:set`'s report of
    // the value written.
    assert_eq!(app.notice_kind(), Some(crate::app::NoticeKind::Refusal));
    let said = app.messages().collect::<Vec<_>>().join("\n");
    assert!(
        said.contains("`x` in `x:block` is no mode")
            && said.contains("— keeping the cursor's shapes"),
        "{said}"
    );
    assert_eq!(shape(&app), Some(4), "the shapes kept");
}
