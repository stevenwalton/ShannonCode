//! **`:themes`** — the list, the preview beside it drawn in the theme under
//! the cursor, and `<CR>` wearing it (Steven, 10-14) — through the keys a
//! user presses and the cells a frame draws.

use super::*;
use crate::style::Ink;
use ratatui::style::Color;
use ratatui::{Terminal, backend::TestBackend};

/// The terminal these tests draw in, and so the one the keys are typed in:
/// the keys were sized 60×20 whatever was drawn (review stop 12, Tier 3).
const FRAME: Rect = Rect::new(0, 0, 120, 40);

fn terminal(w: u16, h: u16) -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(w, h)).unwrap()
}

/// `:themes` typed in the transcript, drawn once.
fn themes(app: &mut App, term: &mut Terminal<TestBackend>) {
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("themes");
    draw_frame(app, term);
}

/// The theme the preview shows.
fn previewed(app: &App) -> Option<String> {
    let pane = app.workspace.first_of(PaneKind::ThemePreview)?;
    app.pane_preview(pane).map(|p| p.name.clone())
}

/// The theme under the list's cursor.
fn under_cursor(app: &App) -> Option<String> {
    let pane = app.workspace.first_of(PaneKind::Page(PageKind::Themes))?;
    app.themes_in(pane)?.1.map(str::to_string)
}

/// Where `kind`'s pane was drawn.
fn rect_of(app: &App, kind: PaneKind, (w, h): (u16, u16)) -> Rect {
    let pane = app.workspace.first_of(kind).expect("shown");
    app.workspace
        .frame_layout(Rect::new(0, 0, w, h))
        .pane(pane)
        .expect("laid out")
        .rect
}

/// **`:themes` is two panes**: the list, focused, its cursor on the theme
/// worn; and beside it the preview, of that theme. `j` moves the cursor and
/// the preview follows it, on the next frame.
#[test]
fn themes_opens_on_the_worn_theme_with_its_preview_beside() {
    let mut app = test_app();
    let mut term = terminal(120, 40);
    themes(&mut app, &mut term);
    assert_eq!(app.focus(), PaneKind::Page(PageKind::Themes));
    assert_eq!(under_cursor(&app).as_deref(), Some("terminal"));
    assert_eq!(previewed(&app).as_deref(), Some("terminal"));
    let list = rect_of(&app, PaneKind::Page(PageKind::Themes), (120, 40));
    let preview = rect_of(&app, PaneKind::ThemePreview, (120, 40));
    assert!(
        list.x < preview.x && list.y == preview.y,
        "side by side: {list:?} {preview:?}"
    );

    type_keys_in(&mut app, FRAME, "j");
    draw_frame(&mut app, &mut term);
    let second = crate::theme::shipped_names()
        .nth(1)
        .expect("premise: more than one");
    assert_eq!(under_cursor(&app).as_deref(), Some(second));
    assert_eq!(previewed(&app).as_deref(), Some(second));
}

/// **The preview pane wears the theme it shows** — its border and its
/// ground, drawn — while the list beside it wears the theme worn. And a
/// theme that leaves the ground unset (`terminal`) is previewed on the
/// terminal's own, not on the worn theme's: the rule `view::reground` keeps.
#[test]
fn the_preview_is_drawn_in_the_theme_under_the_cursor() {
    let mut app = test_app();
    let mut term = terminal(120, 40);
    themes(&mut app, &mut term);
    type_keys_in(&mut app, FRAME, "j");
    draw_frame(&mut app, &mut term);
    // The second shipped theme, read from the list as its siblings read it
    // (review stop 12, T6 13: the name was typed here).
    let second = crate::theme::shipped_names().nth(1).expect("premise: two");
    let (mocha, _) = crate::theme::Theme::load(second, None).unwrap();
    assert_eq!(previewed(&app).as_deref(), Some(second), "premise");
    let ground = mocha.ui(Ink::Background).bg;
    assert!(ground.is_some(), "premise: mocha has a ground");

    let buf = term.backend().buffer().clone();
    let preview = rect_of(&app, PaneKind::ThemePreview, (120, 40));
    let corner = &buf[(preview.x, preview.y)];
    assert_eq!(corner.symbol(), "┌", "premise: the corner");
    assert_eq!(
        corner.style().fg,
        mocha.ui(Ink::Border).fg,
        "its border is the theme's"
    );
    let inside = (
        preview.x + preview.width / 2,
        preview.y + preview.height - 2,
    );
    assert_eq!(buf[inside].style().bg, ground, "its ground is the theme's");
    let list = rect_of(&app, PaneKind::Page(PageKind::Themes), (120, 40));
    assert_eq!(
        buf[(list.x, list.y)].style().fg,
        app.theme().ui(Ink::BorderFocused).fg,
        "the list's border is the worn theme's"
    );
    assert_ne!(
        buf[(list.x + 2, list.y + list.height - 2)].style().bg,
        ground,
        "the list is not on the previewed ground"
    );

    // Wear mocha, then preview `terminal` under it.
    type_keys_in(&mut app, FRAME, "<CR>");
    type_keys_in(&mut app, FRAME, "k");
    draw_frame(&mut app, &mut term);
    assert_eq!(previewed(&app).as_deref(), Some("terminal"), "premise");
    let buf = term.backend().buffer().clone();
    assert_eq!(
        buf[(list.x + 2, list.y + list.height - 2)].style().bg,
        ground,
        "premise: the list is on mocha's ground now"
    );
    assert_eq!(
        buf[inside].style().bg,
        Some(Color::Reset),
        "`terminal` previews on the terminal's own ground"
    );
}

/// **`<CR>` wears the theme under the cursor, through `:set`'s door** —
/// `tui.theme` says it, the app wears it, and the list's mark moves to it.
#[test]
fn enter_wears_the_theme_under_the_cursor() {
    let mut app = test_app();
    let mut term = terminal(120, 40);
    themes(&mut app, &mut term);
    search_for(&mut app, "nord");
    type_keys_in(&mut app, FRAME, "<CR>");
    draw_frame(&mut app, &mut term);
    assert_eq!(app.theme().name(), "nord");
    assert_eq!(app.config.tui.theme, "nord");
    let pane = app
        .workspace
        .first_of(PaneKind::Page(PageKind::Themes))
        .unwrap();
    let page: Vec<String> = app
        .pane_listing(pane)
        .unwrap()
        .lines()
        .iter()
        .map(crate::paint::PageLine::text)
        .collect();
    assert!(page.iter().any(|l| l == " ● nord"), "{page:?}");
    assert!(page.iter().any(|l| l == "   terminal"), "{page:?}");
}

/// `/` then the pattern, then Enter — taken by the search line, never by
/// the list's `<CR>`.
fn search_for(app: &mut App, pattern: &str) {
    type_keys_in(app, FRAME, "/");
    type_keys_in(app, FRAME, pattern);
    type_keys_in(app, FRAME, "<CR>");
}

/// **A search moves to a theme; the list stays whole** (Steven, 10-14:
/// "act like our standard searching", not ghostty's narrowing). And the
/// search's `<CR>` is the search's: nothing is worn.
#[test]
fn a_search_moves_to_a_theme_and_the_list_stays_whole() {
    let mut app = test_app();
    let mut term = terminal(120, 40);
    themes(&mut app, &mut term);
    let pane = app
        .workspace
        .first_of(PaneKind::Page(PageKind::Themes))
        .unwrap();
    let before = app.pane_listing(pane).unwrap().lines().len();
    search_for(&mut app, "dracula");
    assert_eq!(under_cursor(&app).as_deref(), Some("dracula"));
    assert_eq!(app.pane_listing(pane).unwrap().lines().len(), before);
    assert_eq!(app.theme().name(), "terminal", "a search wears nothing");
}

/// **`<CR>` on a line naming no theme says so** — the lead, a heading — and
/// changes nothing. With no theme selected the preview is the theme WORN
/// (Steven, 10-14), not the last one the cursor passed.
#[test]
fn enter_on_a_heading_says_why_and_the_preview_is_the_worn_theme() {
    let mut app = test_app();
    let mut term = terminal(120, 40);
    themes(&mut app, &mut term);
    type_keys_in(&mut app, FRAME, "j");
    draw_frame(&mut app, &mut term);
    assert_eq!(
        previewed(&app).as_deref(),
        crate::theme::shipped_names().nth(1),
        "premise"
    );
    type_keys_in(&mut app, FRAME, "gg");
    draw_frame(&mut app, &mut term);
    assert_eq!(under_cursor(&app), None, "premise: the lead");
    assert_eq!(
        previewed(&app).as_deref(),
        Some("terminal"),
        "no theme selected previews the worn one — not the last passed"
    );
    type_keys_in(&mut app, FRAME, "<CR>");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("no theme on this line")),
        "{:?}",
        app.notice()
    );
    assert_eq!(app.theme().name(), "terminal");
}

/// **The preview is loaded once per name** — its place survives a frame
/// and a resize — and a new name is a new page, at its top.
#[test]
fn the_preview_keeps_its_place_until_the_theme_changes() {
    let mut app = test_app();
    let mut term = terminal(120, 40);
    themes(&mut app, &mut term);
    app.set_focus(PaneKind::ThemePreview);
    type_keys_in(&mut app, FRAME, "5j");
    draw_frame(&mut app, &mut term);
    let pane = app.workspace.first_of(PaneKind::ThemePreview).unwrap();
    let place = |app: &App| app.pane_preview(pane).unwrap().text.cursor();
    assert_eq!(place(&app), 5, "premise: moved");
    draw_frame(&mut app, &mut term);
    assert_eq!(place(&app), 5, "a frame keeps it");
    let mut wider = terminal(140, 40);
    draw_frame(&mut app, &mut wider);
    assert_eq!(place(&app), 5, "a resize keeps it");
    app.set_focus(PaneKind::Page(PageKind::Themes));
    type_keys_in(&mut app, Rect::new(0, 0, 140, 40), "j");
    draw_frame(&mut app, &mut wider);
    assert_eq!(place(&app), 0, "a new theme starts at the top");
}

/// **The preview leaves with its list**, as an inspector with its graph;
/// closed alone, the list stays.
#[test]
fn the_preview_leaves_with_its_list() {
    let mut app = test_app();
    let mut term = terminal(120, 40);
    themes(&mut app, &mut term);
    app.dispatch_command_for_test("q");
    assert!(!app.workspace.has(PaneKind::Page(PageKind::Themes)));
    assert!(
        !app.workspace.has(PaneKind::ThemePreview),
        "the preview went too"
    );

    themes(&mut app, &mut term);
    app.set_focus(PaneKind::ThemePreview);
    app.dispatch_command_for_test("q");
    assert!(
        app.workspace.has(PaneKind::Page(PageKind::Themes)),
        "the list stays"
    );
    assert!(!app.workspace.has(PaneKind::ThemePreview));
}

/// **`:help themes`, typed in the list, opens in the list's pane, and `:q`
/// gives the pair back** — the pointer the list's own status row gives. It
/// opened across the top, as over `:graph`, because the list and its preview
/// were thrown away; set aside now, they come back where they were (Steven,
/// 10-16: a view opens in the pane you are in, from every pane).
#[test]
fn help_typed_in_the_list_takes_its_pane_and_q_gives_the_pair_back() {
    let mut app = test_app();
    let mut term = terminal(120, 40);
    themes(&mut app, &mut term);
    type_keys_in(&mut app, FRAME, "j");
    let list = app.workspace.focused();
    app.open_help_command(Some("themes"));
    draw_frame(&mut app, &mut term);
    assert_eq!(app.focus(), PaneKind::Page(PageKind::Help));
    assert_eq!(app.workspace.focused(), list, "in the list's own pane");
    assert!(
        !app.workspace.has(PaneKind::ThemePreview),
        "its preview set aside with it"
    );
    app.dispatch_command_for_test("q");
    draw_frame(&mut app, &mut term);
    assert_eq!(
        app.focus(),
        PaneKind::Page(PageKind::Themes),
        "`:q` gives it back"
    );
    assert!(app.workspace.has(PaneKind::ThemePreview), "and its preview");
    assert_eq!(
        under_cursor(&app).as_deref(),
        crate::theme::shipped_names().nth(1),
        "where it was"
    );
}

/// **`:help` typed in the preview shows over the list and it, and `:q`
/// gives the pair back** — a companion is its host's window (Steven, 10-26,
/// F 1's (a): "they can sit on top of both"). It took the preview's own
/// pane, and `:q` then left the list with no preview (review stop 10, F 1c).
#[test]
fn help_typed_in_the_preview_shows_over_the_pair_and_q_gives_it_back() {
    let mut app = test_app();
    let mut term = terminal(120, 40);
    themes(&mut app, &mut term);
    let list = app.workspace.focused();
    let preview = (app.workspace.panes_of(PaneKind::ThemePreview).next()).expect("premise: one");
    app.focus_pane(preview);
    app.open_help_command(Some("themes"));
    draw_frame(&mut app, &mut term);
    assert_eq!(
        app.workspace.kind_of(list),
        Some(PaneKind::Page(PageKind::Help)),
        "in the list's window"
    );
    assert_eq!(app.workspace.focused(), list, "focused there");
    assert!(
        !app.workspace.has(PaneKind::ThemePreview),
        "the preview left with the list"
    );
    app.dispatch_command_for_test("q");
    draw_frame(&mut app, &mut term);
    assert_eq!(
        app.focus(),
        PaneKind::Page(PageKind::Themes),
        "`:q` gives the list back"
    );
    assert!(app.workspace.has(PaneKind::ThemePreview), "and a preview");
}

/// **A session shown in the list's pane takes its preview** — `:new` there
/// replaces the list (`enter_session`), and a preview left beside a
/// transcript would describe a list that is not there.
#[test]
fn a_session_over_the_list_takes_its_preview() {
    let mut app = test_app();
    let mut term = terminal(120, 40);
    themes(&mut app, &mut term);
    app.new_session_command();
    draw_frame(&mut app, &mut term);
    assert!(
        !app.workspace.has(PaneKind::Page(PageKind::Themes)),
        "premise: the session took the pane"
    );
    assert!(
        !app.workspace.has(PaneKind::ThemePreview),
        "the preview went with it"
    );
}

/// **An imported theme is listed under where it is from, and `<CR>` wears
/// it by its spaced name** — one value, never re-parsed: `:set
/// tui.theme=Sea Breeze` would read `Breeze` as a second item.
#[test]
fn an_imported_theme_is_listed_and_worn_by_its_spaced_name() {
    let dir = tempfile::tempdir().unwrap();
    let palettes = dir.path().join("palettes");
    std::fs::create_dir(&palettes).unwrap();
    let file: String = (0..16)
        .map(|n| format!("palette = {n}=#1{n:x}2{n:x}3{n:x}\n"))
        .chain(["background = #101820\nforeground = #e0e0e0\n".to_string()])
        .collect();
    std::fs::write(palettes.join("Sea Breeze"), file).unwrap();
    // On the stack, as a file of the user's puts it — `<CR>` is a `:set`,
    // which rebuilds the config from the stack.
    let import = toml::Value::Array(vec![palettes.display().to_string().into()]);
    let conf = dir.path().join("import.toml");
    std::fs::write(&conf, format!("[tui]\ntheme_import = {import}\n")).unwrap();
    let mut app = test_app();
    app.set_theme_env(|_| None);
    app.source_file(shannon_core::config::Origin::Sourced, &conf);
    let mut term = terminal(120, 40);
    themes(&mut app, &mut term);
    let pane = app
        .workspace
        .first_of(PaneKind::Page(PageKind::Themes))
        .unwrap();
    let page: Vec<String> = app
        .pane_listing(pane)
        .unwrap()
        .lines()
        .iter()
        .map(crate::paint::PageLine::text)
        .collect();
    let heading = page
        .iter()
        .position(|l| l.starts_with(" imported — "))
        .unwrap_or_else(|| panic!("{page:?}"));
    let at = page
        .iter()
        .position(|l| l == "   Sea Breeze")
        .expect("listed");
    assert!(heading < at, "{page:?}");
    search_for(&mut app, "Sea Breeze");
    assert_eq!(
        under_cursor(&app).as_deref(),
        Some("Sea Breeze"),
        "{:?}",
        app.notice()
    );
    type_keys_in(&mut app, FRAME, "<CR>");
    assert_eq!(app.theme().name(), "Sea Breeze", "{:?}", app.notice());
}

/// **A theme that cannot be read previews why**, in the worn theme — and
/// `<CR>` on it keeps the worn one, as `:set` does.
#[test]
fn an_unreadable_theme_previews_why() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("themes")).unwrap();
    std::fs::write(dir.path().join("themes/broken.toml"), "keyword = [").unwrap();
    let mut app = test_app();
    app.user_config = Ok(dir.path().join("config.toml"));
    let mut term = terminal(120, 40);
    themes(&mut app, &mut term);
    search_for(&mut app, "broken");
    draw_frame(&mut app, &mut term);
    assert_eq!(previewed(&app).as_deref(), Some("broken"), "premise");
    let pane = app.workspace.first_of(PaneKind::ThemePreview).unwrap();
    let page: String = app
        .pane_preview(pane)
        .unwrap()
        .text
        .lines()
        .iter()
        .map(crate::paint::PageLine::text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(page.contains("could not be read"), "{page}");
    type_keys_in(&mut app, FRAME, "<CR>");
    draw_frame(&mut app, &mut term);
    assert_eq!(app.theme().name(), "terminal", "the worn one stays");
    assert_eq!(
        app.config.tui.theme, "broken",
        "premise: the setting says the other"
    );
    // The mark is on the theme WORN, not the name the setting holds.
    let pane = app
        .workspace
        .first_of(PaneKind::Page(PageKind::Themes))
        .unwrap();
    let page: Vec<String> = app
        .pane_listing(pane)
        .unwrap()
        .lines()
        .iter()
        .map(crate::paint::PageLine::text)
        .collect();
    assert!(page.iter().any(|l| l == " ● terminal"), "{page:?}");
    assert!(page.iter().any(|l| l == "   broken"), "{page:?}");
}

/// **The verdict is the theme's to paint, and a theme that blurs it is told**
/// (Steven, 10-14: "If there's a color defined in our program, we should let
/// someone be able to specify how *they* want that drawn"). It was locked —
/// `Look::Own`, so a theme recoloured the shell and never the answer — but
/// the shipped verdict is the terminal's red and green, whose values the
/// terminal's own theme decides, so the lock was never whole. What stands in
/// its place is said at load: a shell painted in the verdict's colours, and
/// a scale with two steps alike.
#[tokio::test]
async fn a_hostile_theme_is_told_what_it_did() {
    use ratatui::style::{Color, Modifier};
    let drawn = |theme: crate::theme::Theme| {
        let mut app = test_app();
        app.set_theme(theme);
        model_call(
            &mut app,
            "tu_1",
            "mkdir -p build && rg --count 'TODO' src && rm build/old.txt",
        );
        app.set_focus(PaneKind::Tool);
        frame_of(&mut app, 120, 30)
    };
    let cell = |buf: &ratatui::buffer::Buffer, needle: &str| {
        buf[cell_of(buf, needle).unwrap_or_else(|| panic!("{needle} is drawn"))].clone()
    };
    let dir = tempfile::tempdir().unwrap();
    let load = |text: &str| {
        std::fs::write(dir.path().join("t.toml"), text).unwrap();
        crate::theme::Theme::load("t", Some(dir.path())).expect("loads")
    };

    // The shell on the verdict's hues: said, and the verdict stays the
    // theme's own (here, the shipped one's).
    let (theme, said) = load(
        "\"decision.function.call\" = \"red\"\n\"decision.string\" = \"green\"\n\
         \"decision.variable.parameter\" = \"yellow\"\n",
    );
    assert!(said.iter().any(|n| n.contains("its own marks")), "{said:?}");
    let buf = drawn(theme);
    assert_eq!(
        cell(&buf, "ASK").style().fg,
        Some(Color::Yellow),
        "the headline"
    );
    assert_eq!(cell(&buf, "mkdir").style().bg, Some(Color::Yellow));
    assert_eq!(cell(&buf, "rm build").style().bg, Some(Color::Red));
    let rg = cell(&buf, "rg --count");
    assert_eq!(rg.style().fg, Some(Color::Green), "green, on the name");
    assert!(rg.style().add_modifier.contains(Modifier::BOLD));
    assert_eq!(
        cell(&buf, "'TODO'").style().fg,
        Some(Color::Green),
        "the theme's"
    );

    // The verdict itself, repainted: drawn as the theme says, and a scale
    // whose deny is its allow is told so.
    // The fills: two oranges three steps apart, which no eye tells apart.
    let (theme, said) = load(
        "\"ui.caution\" = \"#ff8800\"\n\"ui.danger\" = \"green\"\n\
         \"ui.caution.fill\" = { fg = \"black\", bg = \"#ff8a02\" }\n\
         \"ui.danger.fill\" = { fg = \"black\", bg = \"#ff8800\" }\n",
    );
    for pair in [
        "`ui.safe` and `ui.danger`",
        "`ui.caution.fill` and `ui.danger.fill`",
    ] {
        assert!(said.iter().any(|n| n.contains(pair)), "{pair}: {said:?}");
    }
    let buf = drawn(theme);
    assert_eq!(
        cell(&buf, "ASK").style().fg,
        Some(Color::Rgb(0xff, 0x88, 0))
    );
    assert_eq!(
        cell(&buf, "rm build").style().bg,
        Some(Color::Rgb(0xff, 0x88, 0))
    );
}

/// **A theme reaches the whole screen** (Steven, 10-14: "even the borders
/// (currently a cyan color) should be shown as the new color of the theme")
/// — its ground under every pane and the status row, its border colour on the
/// focused pane, its status-row colour on the row — asserted on the drawn
/// cells, since a role mapped right can still be drawn wrong.
#[test]
fn a_theme_reaches_the_ground_the_borders_and_the_status_row() {
    use ratatui::style::Color;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("t.toml"),
        "\"ui.background\" = { fg = \"#cdd6f4\", bg = \"#1e1e2e\" }\n\
         \"ui.border.focused\" = \"#f5c2e7\"\n\"ui.statusline\" = \"#a6adc8\"\n",
    )
    .unwrap();
    let (theme, said) = crate::theme::Theme::load("t", Some(dir.path())).expect("loads");
    assert!(said.is_empty(), "{said:?}");
    let mut app = test_app();
    app.set_theme(theme);
    app.set_focus(PaneKind::Prompt);
    let buf = frame_of(&mut app, 80, 24);
    let ground = Some(Color::Rgb(0x1e, 0x1e, 0x2e));
    let (w, h) = (buf.area().width, buf.area().height);
    for (x, y) in [(0, 0), (w / 2, h / 2), (w - 1, h - 1), (2, h - 1)] {
        assert_eq!(buf[(x, y)].style().bg, ground, "the ground at ({x}, {y})");
    }
    let pink = Some(Color::Rgb(0xf5, 0xc2, 0xe7));
    let focused_corner = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .any(|(x, y)| buf[(x, y)].symbol() == "┌" && buf[(x, y)].style().fg == pink);
    assert!(focused_corner, "the focused pane's border is the theme's");
    assert_eq!(
        buf[(2, h - 1)].style().fg,
        Some(Color::Rgb(0xa6, 0xad, 0xc8)),
        "the status row's text"
    );
}

/// **An imported palette applies by its file's name — and a test app reads
/// none of this machine's**: `tui.theme_import` names `ghostty` by default,
/// and nothing is listed until launch hands over an environment
/// (`set_theme_env`).
#[test]
fn an_imported_theme_applies_and_a_test_app_reads_none() {
    use ratatui::style::Color;
    let app = test_app();
    assert_eq!(app.config.tui.theme_import, ["ghostty"], "premise");
    assert!(
        app.theme_places().imported.is_empty(),
        "no environment, nothing"
    );

    let dir = tempfile::tempdir().unwrap();
    let file: String = (0..16)
        .map(|n| format!("palette = {n}=#1{n:x}2{n:x}3{n:x}\n"))
        .chain(["background = #101820\nforeground = #e0e0e0\n".to_string()])
        .collect();
    std::fs::write(dir.path().join("Sea Breeze"), file).unwrap();
    let mut config = Config::default();
    config.tui.theme_import = vec![dir.path().display().to_string()];
    config.tui.theme = "Sea Breeze".into();
    let mut app = test_app_with(config);
    app.set_theme_env(|_| None);
    app.load_theme();
    assert_eq!(app.theme().name(), "Sea Breeze");
    assert_eq!(
        app.theme().ui(crate::style::Ink::Background).bg,
        Some(Color::Rgb(0x10, 0x18, 0x20))
    );
}

/// **`:analyze` paints as the decision pane does** — the pane's map, where
/// green is the verdict's, not the transcript's, where a string is green.
#[test]
fn analyze_paints_in_the_decision_panes_colours() {
    use crate::style::Ink;
    use crate::theme::Context;
    let mut app = test_app();
    app.open_analyze_command("echo 'hello there'");
    let buf = frame_of(&mut app, 100, 30);
    // The last drawn: the pane's title names the input too, in its own ink.
    let at = cells_of(&buf, "'hello there'").last();
    let fg = buf[at.expect("the string is drawn")].style().fg;
    let (decision, code) = (
        app.theme().paint(Context::Decision, Ink::String).fg,
        app.theme().paint(Context::Code, Ink::String).fg,
    );
    assert_ne!(decision, code, "premise: the two maps differ here");
    assert_eq!(fg, decision);
}

/// **`tui.theme`, all three halves** (`[[feedback_setting_has_three_halves]]`):
/// read at launch from `themes/` beside your config; read again on every
/// config change, so `:reload` takes an edited theme FILE whose name did not
/// change; and applied to the decision pane already open, whose report was
/// painted when it was built. A name that cannot be read keeps the theme
/// there is, and the screen says so.
#[tokio::test]
async fn the_theme_loads_at_launch_and_repaints_the_open_pane_on_reload() {
    use ratatui::style::Color;
    let dir = tempfile::tempdir().unwrap();
    let mine = dir.path().join("config.toml");
    std::fs::write(&mine, "[tui]\ntheme = \"mine\"\n").unwrap();
    std::fs::create_dir(dir.path().join("themes")).unwrap();
    let file = dir.path().join("themes").join("mine.toml");
    // With a key that names no role, so loading it has something to say.
    let flags_in = |rgb: &str| {
        std::fs::write(
            &file,
            format!("\"decision.variable.parameter\" = \"{rgb}\"\nkeywrd = \"red\"\n"),
        )
        .unwrap();
    };
    let noted = |app: &App| app.messages().filter(|m| m.contains("`keywrd`")).count();
    flags_in("#010203");

    let mut config = Config::default();
    config.tui.theme = "mine".into();
    let mut app = test_app_with(config);
    app.set_config_stack(Default::default(), Some(mine));
    assert_eq!(app.theme().name(), "mine", "launch read it");

    // `mkdir` asks, so the pane opens; `rg` is safe, so its flag wears its
    // syntax (a warning's fill would cover it).
    model_call(&mut app, "tu_1", "mkdir -p build && rg --count x");
    app.set_focus(PaneKind::Tool);
    let flag = |app: &mut App| {
        let buf = frame_of(app, 100, 30);
        buf[cell_of(&buf, "--count").expect("the flag is drawn")]
            .style()
            .fg
    };
    assert_eq!(flag(&mut app), Some(Color::Rgb(1, 2, 3)));
    assert_eq!(noted(&app), 1, "what it could not honour, said at launch");
    // Another message between, or `:messages` folds a repeat into the one
    // before it and this could not see one.
    app.dispatch_command_for_test("trust");

    flags_in("#040506");
    app.dispatch_command_for_test("reload");
    assert_eq!(
        flag(&mut app),
        Some(Color::Rgb(4, 5, 6)),
        "the file changed and the setting did not: `:reload` repaints the open pane"
    );
    assert_eq!(noted(&app), 2, "a changed theme says it again");
    app.dispatch_command_for_test("reload");
    app.dispatch_command_for_test("set tui.scrolloff=3");
    assert_eq!(noted(&app), 2, "the same theme says nothing");
    // A rejected entry added to a theme that resolves the same is said.
    std::fs::write(
        &file,
        "\"decision.variable.parameter\" = \"#040506\"\nkeywrd = \"red\"\nnope = 1\n",
    )
    .unwrap();
    app.dispatch_command_for_test("reload");
    assert!(
        app.messages().any(|m| m.contains("`nope`")),
        "the new note is said"
    );

    app.dispatch_command_for_test("set tui.theme=absent");
    assert_eq!(app.theme().name(), "mine", "kept");
    let refusals = |app: &App| {
        app.messages()
            .filter(|m| m.contains("theme `absent`") && m.contains("keeping theme `mine`"))
            .count()
    };
    assert_eq!(refusals(&app), 1, "said");
    // Once: an unrelated `:set` does not say it again.
    app.dispatch_command_for_test("set tui.scrolloff=4");
    app.dispatch_command_for_test("set tui.scrolloff=5");
    assert_eq!(refusals(&app), 1, "not again");
}
