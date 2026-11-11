//! The `:help` page — the manual, read inside Shannon. What the help buffer
//! holds ([`manual`]) and where a topic lands in it ([`anchor`]); the buffer
//! and its windows are `crate::page`'s.
//!
//! `:help` used to shell out to `$EDITOR`, a deliberate placeholder from before
//! panes existed: there was nowhere to *put* a manual that did not mean covering
//! the conversation you were consulting it about. That is exactly what a pane
//! solves, so the placeholder is gone. Reading the manual beside your work —
//! and running the command it describes without closing it — is the whole point
//! of a help system, and it is what vim's own `:help` does.
//!
//! Styling follows vim's help conventions, which are not decoration: a `*tag*`
//! is a definition, a `|link|` is a reference to one, and a header names a
//! section. Colouring the three differently is what lets you skim a page for
//! the one line you need. The link spans are also what `<C-]>` follows
//! (`help::link_at`), and `<C-t>` comes back by the window's tag stack
//! (`page::TagFrom`).

use ratatui::style::Style;

use crate::paint::{PageLine, Run};
use crate::style::{CellStyle, Ink};
use crate::viewport::Place;

/// **The manual's text, composed once** — from the command table and the
/// default config (`options_chapter`), both fixed for the process, so there
/// is nothing a second composition could say differently. Every `:help` used
/// to compose all of it again to read one anchor.
fn page() -> &'static str {
    static PAGE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PAGE.get_or_init(|| crate::help::compose(&crate::command::commands_chapter()))
}

/// The whole manual, styled — what the help buffer holds.
pub(crate) fn manual() -> Vec<PageLine> {
    style_page(page())
}

/// `:help <arg>` — **the source line of the manual `arg` names**: a chapter,
/// any `*tag*` in the text, a `:command` or an option (`help::anchor_line`).
/// `None` only when none of those resolve, which the caller turns into a
/// refusal naming them.
///
/// One page and one anchor, because a chapter *is* a tag — the one its
/// heading declares. Composing a separate page per chapter gave chapters the
/// only behaviour with no anchor at all, so `:help config` landed on the
/// manual's general blurb with the chapter below the fold. It also cut the
/// page off from its own `|links|`: following one out of a self-contained
/// chapter had nowhere to go.
pub(crate) fn anchor(name: &str) -> Option<usize> {
    crate::help::anchor_line(page(), name)
}

/// **Where `:help [name]` opens the manual** — at the top with no name; with
/// one, the line it names at the top of the window (vim's tag jump); `Err`
/// the refusal naming what it could have meant (`help::no_such_topic`).
/// Asked before the help buffer is made, so a refusal makes nothing.
pub(crate) fn opening(name: Option<&str>) -> Result<Option<(usize, Place)>, String> {
    let Some(name) = name else {
        return Ok(None);
    };
    anchor(name)
        .map(|line| Some((line, Place::Top)))
        .ok_or_else(|| crate::help::no_such_topic(name))
}

/// Style one page: headers, `*tags*`, `|links|`, `` `literals` ``, keys and
/// a key table's keys — each a role (`ui.help.*`), painted at the draw, so
/// a theme reaches the manual.
fn style_page(page: &str) -> Vec<PageLine> {
    let cells = crate::help::key_cells(page.lines());
    (page.lines().zip(cells))
        .map(|(line, cell)| style_manual(line, cell.map(|cell| cell.at)))
        .collect()
}

/// **One line of the manual, by its shape**: a rule faint, a heading a
/// heading, the rest prose — its marks and `cell` in any of them. The
/// manual's own: a lesson says what it is itself (`tutor::Kind`).
fn style_manual(line: &str, cell: Option<std::ops::Range<usize>>) -> PageLine {
    use crate::help::{Dialect, marks};
    if line.starts_with("===") {
        return PageLine {
            runs: vec![Run::role(line, CellStyle::ink(Ink::TextFaint))],
            style: Style::default(),
        };
    }
    let base = match crate::help::is_heading(line) {
        true => CellStyle::ink(Ink::HelpHeading).bold(),
        false => CellStyle::PLAIN,
    };
    style_marked(line, marks(line, Dialect::Manual), cell, base)
}

/// One line of the manual, read as a table of one — the tests'; a page is
/// read whole (`style_page`), its tables with it.
#[cfg(test)]
pub(crate) fn style_line(line: &str) -> PageLine {
    style_manual(line, crate::help::key_cell_alone(line))
}

/// **One line painted from its marks** on `base` — as [`crate::help::marks`]
/// read them, the manual's a line at a time and the tutor's a paragraph at a
/// time (`crate::tutor::lines`), and **a key table's keys as keys**: `cell`,
/// where on the line its table's keys are (`help::key_cells`, the cells
/// `:help` finds a key in) — `CTRL-W s`, `zo zO`, `:graph` at the head of a
/// row are what you type, and drew as prose. `marks` in order, each within
/// `line`.
pub(crate) fn style_marked(
    line: &str,
    marks: Vec<(std::ops::Range<usize>, crate::help::Mark)>,
    cell: Option<std::ops::Range<usize>>,
    base: CellStyle,
) -> PageLine {
    use crate::help::Mark;
    let what_you_type = CellStyle::ink(Ink::HelpCode);
    let marks = marks.into_iter().filter(|(at, _)| {
        // Inside the key cell, the cell's own look.
        cell.as_ref()
            .is_none_or(|cell| at.end <= cell.start || at.start >= cell.end)
    });
    let marked = cell.iter().map(|cell| (cell.clone(), what_you_type));
    let mut spans: Vec<(std::ops::Range<usize>, CellStyle)> = (marks.map(|(at, mark)| {
        let look = match mark {
            Mark::Tag => CellStyle::ink(Ink::HelpTag).bold(),
            Mark::Link => CellStyle::ink(Ink::HelpRef),
            Mark::Literal | Mark::Key => what_you_type,
            Mark::Emphasis { strong: true } => base.bold(),
            Mark::Emphasis { strong: false } => base.italic(),
        };
        (at, look)
    }))
    .chain(marked)
    .collect();
    spans.sort_by_key(|(at, _)| at.start);
    let mut runs = Vec::new();
    let mut from = 0;
    for (at, look) in spans {
        if at.start > from {
            runs.push(Run::role(&line[from..at.start], base));
        }
        runs.push(Run::role(&line[at.clone()], look));
        from = at.end;
    }
    if from < line.len() {
        runs.push(Run::role(&line[from..], base));
    }
    PageLine {
        runs,
        style: Style::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;
    use ratatui::text::Line;

    fn plain(line: &PageLine) -> String {
        line.text()
    }

    /// A help buffer, as the App makes one.
    fn help() -> crate::page::PageBuffer {
        let id = crate::wm::BufferId(0);
        crate::page::PageBuffer::of(id, crate::page::Recipe::Manual)
    }

    /// The window `:help name` opens on it.
    fn window_at(help: &crate::page::PageBuffer, name: &str) -> Option<crate::page::PageWindow> {
        let at = opening(Some(name)).ok()?;
        Some(help.window(Some(name), at))
    }

    /// A page line as the shipped theme draws it.
    fn painted(line: &PageLine) -> Line<'static> {
        let theme = crate::theme::Theme::terminal();
        crate::paint::Painter::new(&theme, crate::theme::Context::Code).line(line)
    }

    /// Styling must never alter the text — the width contract is enforced on
    /// the composed page, and a renderer that adds or drops a character would
    /// silently break it.
    #[test]
    fn styling_preserves_every_line_verbatim() {
        let page = crate::help::compose(&crate::command::commands_chapter());
        let styled = style_page(&page);
        let back: Vec<String> = styled.iter().map(plain).collect();
        assert_eq!(back, page.lines().collect::<Vec<_>>());
    }

    /// **Every chapter opens with its heading on screen**, at every pane size
    /// worth caring about — including the LAST chapter, where the anchor is
    /// past `max_scroll` and clamping decides what you see.
    ///
    /// The jump is held until the width is known and resolved by `clamp`, so
    /// this drives that path rather than reading the offset straight after
    /// `window_at` (where it is deliberately still 0).
    #[test]
    fn every_chapter_opens_with_its_heading_visible() {
        let help = help();
        for t in crate::help::TOPICS {
            let want = anchor(t.name).expect("resolves");
            for (w, h) in [(80u16, 40usize), (40, 20), (100, 12)] {
                let mut view = window_at(&help, t.name).expect("opens");
                view.text_mut().clamp(h, w, 0);
                let scroll = view.text().scroll() as usize;
                let row = view.text().rows_before(want, w);
                assert!(
                    row >= scroll && row < scroll + h,
                    "{}: heading at row {row} not visible in {scroll}..{} ({w}x{h})",
                    t.name,
                    scroll + h
                );
            }
        }
    }

    /// …and when there is room below it, the heading is at the TOP — vim's
    /// tag-jump placement, not merely "somewhere on screen".
    #[test]
    fn a_chapter_with_room_below_it_opens_at_the_very_top() {
        let (w, h) = (80u16, 24usize);
        let first = crate::help::TOPICS.first().expect("a chapter");
        let want = anchor(first.name).expect("resolves");
        let mut view = window_at(&help(), first.name).expect("opens");
        view.text_mut().clamp(h, w, 0);
        assert_eq!(
            view.text().scroll() as usize,
            view.text().rows_before(want, w),
            "the heading is the first row, not the tenth line of the blurb"
        );
    }

    #[test]
    fn tags_links_and_literals_get_their_own_style() {
        let line = painted(&style_line("see |graph| and *tag* and `:graph`"));
        let styled: Vec<&str> = line
            .spans
            .iter()
            .filter(|s| s.style.fg.is_some())
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(
            styled,
            vec!["|graph|", "*tag*", "`:graph`"],
            "each marked run is styled, delimiters included"
        );
        // …each its OWN: `fg.is_some()` passed with all three in one role
        // (review stop 12, T7).
        let fg = |text: &str| {
            (line.spans.iter())
                .find(|s| s.content == text)
                .map(|s| s.style.fg)
        };
        let (link, tag, literal) = (fg("|graph|"), fg("*tag*"), fg("`:graph`"));
        assert!(
            link != tag && tag != literal && link != literal,
            "three runs, three colours: {link:?} {tag:?} {literal:?}"
        );
    }

    /// **What you type is drawn as what you type** — a key table's keys
    /// (`CTRL-W s` heading its row) and a key written in prose (`<Esc>`)
    /// in the literal's colour, the row's words plain; a table of words
    /// (`grep` and `rg`) plain throughout. Drawn, not read off the marks: a
    /// role is only right once it is painted (Steven, 11-08: "make sure to
    /// properly paint the commands in help files").
    #[test]
    fn keys_are_drawn_as_what_you_type() {
        let page =
            "    CTRL-W s     split it, then <Esc>\n      grep   a read\n      rg     a read\n";
        let lines: Vec<Line> = style_page(page).iter().map(painted).collect();
        let fg = |line: &Line, text: &str| {
            (line.spans.iter())
                .find(|s| s.content == text)
                .unwrap_or_else(|| panic!("{text:?} in {line:?}"))
                .style
                .fg
        };
        let literal = painted(&style_line("`x`")).spans[0].style.fg;
        assert!(literal.is_some(), "premise: a literal has a colour");
        assert_eq!(fg(&lines[0], "    "), None, "premise: the indent is plain");
        assert_eq!(fg(&lines[0], "CTRL-W s"), literal, "the row's keys");
        assert_eq!(fg(&lines[0], "<Esc>"), literal, "a key in prose");
        assert!(
            lines[2].spans.iter().all(|s| s.style.fg.is_none()),
            "{:?}",
            lines[2]
        );
    }

    /// The tutor's emphasis is drawn as emphasis, never as a tag.
    #[test]
    fn the_tutors_emphasis_is_drawn_as_emphasis() {
        let text = "works *with* you";
        let marks = crate::help::marks(text, crate::help::Dialect::Tutor);
        let line = painted(&style_marked(text, marks, None, CellStyle::PLAIN));
        let with = line
            .spans
            .iter()
            .find(|s| s.content == "*with*")
            .expect("a run");
        assert!(
            with.style.add_modifier.contains(Modifier::ITALIC),
            "{with:?}"
        );
        assert_eq!(with.style.fg, None, "no tag's colour");
    }

    /// A rule is structure, drawn apart from prose (review stop 17, C 9).
    #[test]
    fn a_rule_is_drawn_faint() {
        let (rule, prose) = (
            painted(&style_line("==========")),
            painted(&style_line("words here")),
        );
        assert_ne!(rule.spans[0].style, prose.spans[0].style);
    }

    #[test]
    fn a_heading_is_styled_but_indented_prose_is_not() {
        assert!(
            painted(&style_line("THE DECISION")).spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            !painted(&style_line("    plain indented prose")).spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD),
        );
    }

    /// The manual is full of em dashes and arrows; slicing by byte offset must
    /// land on character boundaries or this panics.
    #[test]
    fn multibyte_lines_do_not_panic() {
        for line in [
            "— a dash before |a-link| and after —",
            "世界 `code` 世界",
            "*tag* — 🦀 — |link|",
            "unbalanced * marker",
            "empty ** pair",
        ] {
            assert_eq!(plain(&style_line(line)), line);
        }
    }

    #[test]
    fn a_topic_resolves_and_an_unknown_one_does_not() {
        assert!(anchor("windows").is_some());
        assert!(anchor("subagent").is_some(), "aliases too");
        assert!(anchor("nonsense").is_none());
        assert_eq!(opening(None), Ok(None), "no name is the top");
        assert!(opening(Some("nonsense")).is_err());
    }
}
