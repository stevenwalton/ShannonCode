//! **`:themes`** — every theme there is, and what the one under the cursor
//! looks like (Steven, 10-14: "What I like about [ghostty's list] is that it
//! shows you what the theme looks like … But what I don't like is that you
//! can't just select it right then and there").
//!
//! Two panes, as `:graph` and its inspector are: a page listing the themes —
//! an ordinary text page, so `/`, `?`, `n`, `N` and every motion are the ones
//! every page has, and a search MOVES to a theme rather than narrowing the
//! list — and beside it a preview, a page drawn in the theme under the
//! cursor: two transcripts, a prompt, the status row, a decision, and the
//! rest. `<CR>` on a theme wears it, through `:set`'s door.
//!
//! The list is read when `:themes` is typed, and kept by its page buffer
//! (`page::Recipe::Themes`): ghostty's directory is 463 files, and listing it
//! every frame — or at a split of the list — would be disk on the draw path.
//! The preview is rebuilt only when the theme under the cursor, or the width,
//! changes.

use ratatui::style::{Modifier, Style};

use crate::page::Lines;
use crate::paint::{PageLine, Run};
use crate::style::{CellStyle, Ink};
use crate::theme::{Context, Source, Theme};
use crate::wrap::{clip, clip_middle, str_cells};

/// **The themes `:themes` lists**, as their places were read. Which page
/// line is which theme is the PAGE's to say — each theme's line names it
/// (`page::Row::Theme`), as every listing's rows name theirs (V5.2); this kept a
/// column of its own for it until then.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ThemeList {
    entries: Vec<(String, Source)>,
    /// The theme the page was last built marking ([`page`](Self::page)).
    marked: Option<String>,
}

/// **The preview pane's state**: the theme it shows, loaded once per name,
/// and its page at the width it was built for.
#[derive(Debug)]
pub(crate) struct Preview {
    pub(crate) name: String,
    /// What the pane is drawn in — its page, ground and border: the theme,
    /// or the worn one when it could not be read, under what loading said.
    pub(crate) theme: Theme,
    place: String,
    notes: Vec<String>,
    failed: Option<String>,
    width: usize,
    pub(crate) text: crate::text_view::TextView,
}

impl Preview {
    /// The preview of `name`, from `place`, as `loaded` (`Theme::load_from`)
    /// left it — drawn in `worn` with the reason when it could not be read.
    pub(crate) fn new(
        name: String,
        place: String,
        loaded: Result<(Theme, Vec<String>), String>,
        worn: &Theme,
        width: usize,
    ) -> Preview {
        let (theme, notes, failed) = match loaded {
            Ok((theme, notes)) => (theme, notes, None),
            Err(why) => (worn.clone(), Vec::new(), Some(why)),
        };
        let mut preview = Preview {
            name,
            theme,
            place,
            notes,
            failed,
            width,
            text: crate::text_view::TextView::new(Vec::<PageLine>::new()),
        };
        preview.text = crate::text_view::TextView::new(preview.page());
        preview
    }

    /// **The page at `width`**, rebuilt only when that changed — and set
    /// into the view, so the reader's place survives a resize.
    pub(crate) fn fit(&mut self, width: usize) {
        if width != self.width {
            self.width = width;
            let page = self.page();
            self.text.set_lines(page);
        }
    }

    fn page(&self) -> Vec<PageLine> {
        match &self.failed {
            None => preview(
                &self.name,
                &self.place,
                &self.theme,
                &self.notes,
                self.width,
            ),
            Some(why) => [
                vec![(
                    format!(" {}", self.name),
                    Style::default().add_modifier(Modifier::BOLD),
                )],
                vec![(format!(" {}", self.place), self.theme.ui(Ink::TextQuiet))],
                vec![],
                vec![(
                    format!(" could not be read: {why}"),
                    self.theme.ui(Ink::Caution),
                )],
            ]
            .into_iter()
            .map(line)
            .collect(),
        }
    }
}

/// The page's lead: what the keys do.
const LEAD: &str = " <CR> wears the theme under the cursor, for this run — `/` finds one";

impl ThemeList {
    /// The list of `entries` (`theme::Places::themes`), **grouped by place**:
    /// shipped, yours, then each imported directory in the order given. A
    /// file of yours named for a shipped theme keeps the shipped one's place
    /// in `entries` — it is what the name finds — and would have split the
    /// shipped run under a heading of its own; it is listed with yours.
    pub(crate) fn new(mut entries: Vec<(String, Source)>) -> ThemeList {
        let mut dirs: Vec<std::path::PathBuf> = Vec::new();
        for (_, source) in &entries {
            if let Source::Imported(dir) = source
                && !dirs.contains(dir)
            {
                dirs.push(dir.clone());
            }
        }
        // Stable: within a place, the order `themes` gave.
        entries.sort_by_key(|(_, source)| match source {
            Source::Shipped => 0,
            Source::Yours => 1,
            Source::Imported(dir) => 2 + dirs.iter().position(|d| d == dir).unwrap_or(0),
        });
        ThemeList {
            entries,
            marked: None,
        }
    }

    /// **The page marking `current`, if it is not the one last built.**
    /// The entries are fixed once read, so the page is a function of the
    /// theme worn alone: it is rebuilt when that changes, not every frame —
    /// building it every frame cost 41 µs with ghostty's 463 themes listed,
    /// most of a frame's derive (measured, release).
    pub(crate) fn page(&mut self, current: &str) -> Option<Lines> {
        if self.marked.as_deref() == Some(current) {
            return None;
        }
        self.marked = Some(current.to_string());
        Some(self.lines(current))
    }

    /// **The page**: a heading per place, a line per theme naming it,
    /// `current` — the one worn — marked. In roles, so it is drawn in the
    /// theme worn, which a `<CR>` changes under it.
    fn lines(&self, current: &str) -> Lines {
        let role = |text: String, cell: CellStyle| PageLine {
            runs: vec![Run::role(text, cell)],
            style: Style::default(),
        };
        let heading = CellStyle::ink(Ink::ListingHeading).bold();
        let mut out = Lines::default();
        out.push(None, role(LEAD.to_string(), CellStyle::ink(Ink::TextQuiet)));
        out.push(None, PageLine::default());
        let mut group: Option<&Source> = None;
        for (name, source) in &self.entries {
            if group != Some(source) {
                if group.is_some() {
                    out.push(None, PageLine::default());
                }
                out.push(None, role(format!(" {}", place(source)), heading));
                group = Some(source);
            }
            let line = match name == current {
                true => role(format!(" ● {name}"), CellStyle::ink(Ink::Chosen)),
                false => role(format!("   {name}"), CellStyle::PLAIN),
            };
            out.push(Some(crate::page::Row::Theme(name.clone())), line);
        }
        out
    }

    /// Where the theme `name` is from, if it is listed.
    pub(crate) fn source_of(&self, name: &str) -> Option<&Source> {
        (self.entries.iter())
            .find(|(listed, _)| listed == name)
            .map(|(_, source)| source)
    }
}

/// **Where a theme is from, in words** — the list's heading over it, and the
/// preview's line under its name.
pub(crate) fn place(source: &Source) -> String {
    match source {
        Source::Shipped => "shipped".to_string(),
        Source::Yours => "yours — themes/ beside your config".to_string(),
        // As you would type it (`app::tilde`), the middle cut.
        Source::Imported(dir) => format!("imported — {}", clip_middle(&crate::app::tilde(dir), 60)),
    }
}

/// A page line of styled runs — every style the candidate theme's, so the
/// page paints as itself whatever theme draws the pane around it.
fn line(runs: Vec<(String, Style)>) -> PageLine {
    PageLine {
        runs: runs
            .into_iter()
            .map(|(text, style)| Run::own(text, style))
            .collect(),
        style: Style::default(),
    }
}

/// A row of a sample: its runs.
type Row = Vec<(String, Style)>;

/// `row` in at most `width` cells, cut with `…`.
fn cut(row: &[(String, Style)], width: usize) -> Row {
    let mut out = Vec::new();
    let mut used = 0;
    for (text, style) in row {
        let room = width.saturating_sub(used);
        if room == 0 {
            break;
        }
        let cut = clip(text, room);
        used += str_cells(&cut);
        out.push((cut, *style));
    }
    out
}

/// `row` in exactly `width` cells — [`cut`], or padded.
fn fit(row: &[(String, Style)], width: usize) -> Row {
    let mut out = cut(row, width);
    let used: usize = out.iter().map(|(text, _)| str_cells(text)).sum();
    if used < width {
        out.push((" ".repeat(width - used), Style::default()));
    }
    out
}

/// **A sample pane**: `rows` boxed in `border`'s style, `title` on its top,
/// `width` cells wide in all.
fn boxed(title: &str, rows: &[Row], width: usize, border: Style) -> Vec<Row> {
    let inner = width.saturating_sub(2);
    let title = clip(&format!(" {title} "), inner);
    let top = format!(
        "┌{title}{}┐",
        "─".repeat(inner.saturating_sub(str_cells(&title)))
    );
    let mut out = vec![vec![(top, border)]];
    for row in rows {
        let mut line = vec![("│".to_string(), border)];
        line.extend(fit(row, inner));
        line.push(("│".to_string(), border));
        out.push(line);
    }
    out.push(vec![(format!("└{}┘", "─".repeat(inner)), border)]);
    out
}

/// Two samples side by side, the shorter padded to the taller.
fn beside(left: Vec<Row>, right: Vec<Row>, widths: (usize, usize)) -> Vec<Row> {
    let rows = left.len().max(right.len());
    (0..rows)
        .map(|i| {
            let mut row = fit(left.get(i).map_or(&[][..], |r| r), widths.0);
            row.extend(fit(right.get(i).map_or(&[][..], |r| r), widths.1));
            row
        })
        .collect()
}

/// **What the theme `name` looks like**, in it: the tab line (yours, one
/// with news, one with a decision waiting), two transcripts — your
/// message in its frame and a reply with its markdown and code, and a tool's
/// record — a prompt, the status
/// row, a notice and a question, a decision with both its fills, then the
/// terminal's sixteen as the theme says them, `:graph`'s lanes and marks, the
/// manual's markup, and what loading it said. `width` cells wide; every
/// style the theme's own, so the pane draws it as this theme would.
pub(crate) fn preview(
    name: &str,
    source: &str,
    theme: &Theme,
    notes: &[String],
    width: usize,
) -> Vec<PageLine> {
    let width = width.max(20);
    let code = |ink: Ink| theme.style(Context::Code, CellStyle::ink(ink));
    let decision = |ink: Ink| theme.style(Context::Decision, CellStyle::ink(ink));
    let keys = crate::paint::Painter::new(theme, Context::Decision);
    let ui = |ink: Ink| theme.ui(ink);
    let plain = Style::default();
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let t = |text: &str, style: Style| (text.to_string(), style);
    // What came of a call, as a record draws it.
    let output = theme.style(Context::Code, crate::tool_record::OUTPUT);

    // Your message, framed as a transcript frames it (`Output::begin_turn`),
    // its rules the reply box's inner width.
    let rule = "─".repeat((width / 2).saturating_sub(2));
    let reply: Vec<Row> = vec![
        vec![t(&rule, ui(Ink::BorderUser))],
        vec![t(
            "> how is the config read?",
            Style::default().add_modifier(Modifier::DIM),
        )],
        vec![t(&rule, ui(Ink::BorderUser))],
        vec![],
        vec![t("## Reading the config", code(Ink::Heading))],
        vec![
            t("The ", plain),
            t("`load`", code(Ink::Raw)),
            t(" step reads ", plain),
            t("every", bold),
            t(" layer.", plain),
        ],
        vec![t("```rust", code(Ink::RawFence))],
        vec![
            t("fn ", code(Ink::Keyword)),
            t("load", code(Ink::Function)),
            t("(path: ", plain),
            t("&Path", code(Ink::Type)),
            t(") -> ", plain),
            t("Config", code(Ink::Type)),
            t(" {", plain),
        ],
        vec![
            t("    let ", code(Ink::Keyword)),
            t("depth = ", plain),
            t("8", code(Ink::Number)),
            t("; ", plain),
            t("// bounded", code(Ink::Comment)),
        ],
        vec![
            t("    read(", plain),
            t("\"config.toml\"", code(Ink::String)),
            t(")", plain),
        ],
        vec![t("}", plain)],
        vec![t("```", code(Ink::RawFence))],
    ];
    let record: Vec<Row> = vec![
        vec![
            t("⚙ Bash ran: ", code(Ink::Tool)),
            t("rg", code(Ink::FunctionCall)),
            t(" ", plain),
            t("--count", code(Ink::Parameter)),
            t(" ", plain),
            t("'TODO'", code(Ink::String)),
            t(" src", plain),
        ],
        vec![t("src/app.rs:12", output)],
        vec![t("src/theme.rs:3", output)],
        vec![t("⚙ Write: src/theme.rs", code(Ink::Tool))],
        vec![t("+ let theme = load(name)?;", code(Ink::Added))],
        vec![t("- let theme = Theme::default();", code(Ink::Removed))],
        vec![t("warning: unused import", code(Ink::Warning))],
        vec![t("error: no such file", code(Ink::Error))],
    ];
    // A secret is the one thing the prompt colours; an alias is drawn as
    // typed, as the prompt draws it (`view::row_secrets`).
    let prompt: Vec<Row> = vec![vec![
        t("deploy with ", plain),
        t("{{API_TOKEN}}", ui(Ink::Secret)),
        t(", then tell ", plain),
        t("{{ops}}", plain),
    ]];

    // The tab line, as `view::draw_tabline` draws it: the tab you are in
    // picked out (`ui.tabline.selected`), one with news wearing the alerts'
    // default mark, one where a decision waits in the hue of an ask, the
    // ground the line's (`ui.tabline`) — Steven, 10-26: "update the theme
    // preview to show what tabs look like".
    let labels = [
        (" 1 auth-fix ".to_string(), ui(Ink::TabLineSelected)),
        (
            format!(" 2 {} docs ", shannon_core::config::DEFAULT_ALERT_GLYPH),
            ui(Ink::TabLine),
        ),
        (" 3 1a2b3c4d ".to_string(), ui(Ink::Caution)),
    ];
    let used: usize = labels.iter().map(|(l, _)| crate::wrap::str_cells(l)).sum();
    let mut tabline: Row = labels.into_iter().collect();
    tabline.push((" ".repeat(width.saturating_sub(used)), ui(Ink::TabLine)));

    let mut rows: Vec<Row> = vec![
        vec![
            t(&format!(" {name}"), bold),
            t(&format!("  {source}"), ui(Ink::TextQuiet)),
        ],
        vec![],
        tabline,
    ];
    let reply = boxed(
        "output — a reply",
        &reply,
        width / 2,
        ui(Ink::BorderFocused),
    );
    let record = boxed(
        "output — a tool ran",
        &record,
        width - width / 2,
        ui(Ink::Border),
    );
    if width >= 60 {
        rows.extend(beside(reply, record, (width / 2, width - width / 2)));
    } else {
        rows.extend(reply);
        rows.extend(record);
    }
    rows.extend(boxed(
        "prompt → 1a2b3c4d",
        &prompt,
        width,
        ui(Ink::BorderAimed),
    ));
    rows.push(vec![
        t(" [NORMAL] prompt · sonnet · ctx ", ui(Ink::Statusline)),
        t("72%", ui(Ink::Caution).add_modifier(Modifier::BOLD)),
        t(" · tok ↑1200 ↓300", ui(Ink::Statusline)),
    ]);
    rows.push(vec![]);
    rows.push(vec![
        t(
            " a refusal says why ",
            ui(Ink::Danger).add_modifier(Modifier::BOLD),
        ),
        t("  ", plain),
        t(
            " a report says what happened ",
            ui(Ink::Caution).add_modifier(Modifier::BOLD),
        ),
    ]);
    rows.push(vec![
        t(" ", plain),
        t("WARNING", ui(Ink::DangerSign)),
        t(" ", plain),
        t("delete", ui(Ink::Danger).add_modifier(Modifier::BOLD)),
        t(" ", plain),
        t("3 sessions", ui(Ink::Operand).add_modifier(Modifier::BOLD)),
        t(" permanently? ", ui(Ink::TextQuiet)),
        t("(y/N)", ui(Ink::Text).add_modifier(Modifier::BOLD)),
    ]);
    rows.push(vec![]);
    let pane: Vec<Row> = vec![
        vec![
            t("ASK  ", ui(Ink::Caution).add_modifier(Modifier::BOLD)),
            t(
                " you'll be prompted first",
                Style::default().add_modifier(Modifier::DIM),
            ),
        ],
        vec![],
        vec![
            t("  ", plain),
            t("rg", ui(Ink::Safe).add_modifier(Modifier::BOLD)),
            t(" ", plain),
            t("--count", decision(Ink::Parameter)),
            t(" ", plain),
            t("'TODO'", decision(Ink::String)),
            t(" src ", plain),
            t("&&", decision(Ink::Operator)),
            t(" ", plain),
            t("mkdir -p build", theme.ui(Ink::CautionFill)),
            t(" ", plain),
            t("&&", decision(Ink::Operator)),
            t(" ", plain),
            t("rm build/old.txt", theme.ui(Ink::DangerFill)),
        ],
        vec![
            t("  echo ", plain),
            t("$HOME", decision(Ink::VariableSpecial)),
        ],
        vec![],
        // Its keys drawn as the pane's legend draws them (`paint::hint`).
        std::iter::once(t("answer: ", code(Ink::Tool)))
            .chain(
                crate::paint::hint("d/q deny · a allow once · e edit", CellStyle::PLAIN)
                    .iter()
                    .map(|run| t(&run.text, keys.style(run))),
            )
            .collect(),
    ];
    rows.extend(boxed("run this?", &pane, width, ui(Ink::BorderFocused)));
    rows.push(vec![]);

    rows.push(vec![t(
        " the terminal's sixteen, as this theme says them",
        ui(Ink::TextQuiet),
    )]);
    for half in [0u8, 8] {
        let mut row = vec![t(" ", plain)];
        for slot in half..half + 8 {
            row.push(t("██", Style::default().fg(theme.ansi(slot))));
            row.push(t(&format!("{slot:<3}"), ui(Ink::TextFaint)));
        }
        rows.push(row);
    }
    rows.push(vec![]);
    let mut lanes = vec![t(" :graph  ", ui(Ink::TextQuiet))];
    for lane in Ink::LANES {
        lanes.push(t("● ", ui(lane)));
    }
    lanes.extend([
        t(" ▶ running", ui(Ink::GraphRunning)),
        t("  ‖ held", ui(Ink::GraphHeld)),
    ]);
    rows.push(lanes);
    rows.push(vec![
        t(" ", plain),
        t(" a marked row ", ui(Ink::GraphMark)),
        t("  ", plain),
        t("a turn shared by reference", ui(Ink::GraphShared)),
    ]);
    rows.push(vec![
        t(" ", plain),
        t(" chosen ", ui(Ink::MenuSelected)),
        t("  theme_import  ", ui(Ink::TextQuiet)),
        t(" a screen's bar ", ui(Ink::Bar)),
        t(" ", plain),
        t(" a decision waits ", ui(Ink::BarWaiting)),
    ]);
    rows.push(vec![
        t(
            " THEMES ",
            ui(Ink::HelpHeading).add_modifier(Modifier::BOLD),
        ),
        t("*themes* ", ui(Ink::HelpTag).add_modifier(Modifier::BOLD)),
        t("|set| ", ui(Ink::HelpRef)),
        t("`:set` ", ui(Ink::HelpCode)),
        t("~", ui(Ink::NonText)),
    ]);
    // Every sample row in the width: a wrapped sample is no sample of
    // anything. What loading said is a message, and wraps.
    let mut page: Vec<PageLine> = rows.into_iter().map(|row| line(cut(&row, width))).collect();
    if !notes.is_empty() {
        page.push(PageLine::default());
        page.push(line(vec![t(" what loading it said:", ui(Ink::TextQuiet))]));
        for note in notes {
            page.push(line(vec![t(&format!(" {note}"), ui(Ink::Caution))]));
        }
    }
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> ThemeList {
        ThemeList::new(vec![
            ("terminal".into(), Source::Shipped),
            ("nord".into(), Source::Shipped),
            ("mine".into(), Source::Yours),
            ("Sea Breeze".into(), Source::Imported("/t".into())),
        ])
    }

    /// The theme each page line names — `None` for a heading, the lead, a
    /// blank.
    fn named(page: &Lines) -> Vec<Option<String>> {
        (0..page.lines().len())
            .map(|at| match page.row_at(at) {
                Some(crate::page::Row::Theme(name)) => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    /// **A heading per place, a line per theme**, and each line names which
    /// theme it is — a heading none.
    #[test]
    fn the_list_says_where_each_theme_is_from() {
        let list = list();
        let lines = list.lines("nord");
        let page: Vec<String> = lines.lines().iter().map(PageLine::text).collect();
        let names = named(&lines);
        let at = |name: &str| {
            (names.iter())
                .position(|n| n.as_deref() == Some(name))
                .expect(name)
        };
        assert!(
            page[at("nord")].contains("● nord"),
            "the worn one: {page:?}"
        );
        assert!(!page[at("mine")].contains('●'), "{page:?}");
        assert_eq!(page[at("Sea Breeze")], "   Sea Breeze");
        for (i, text) in page.iter().enumerate() {
            if text.contains("shipped") || text.contains("yours") || text.contains("imported") {
                assert_eq!(names[i], None, "a heading is no theme: {text}");
            }
        }
        assert!(page.iter().any(|l| l.contains("imported — /t")), "{page:?}");
        assert_eq!(list.source_of("mine"), Some(&Source::Yours));
    }

    /// **The page is rebuilt only when the theme worn changes** — every frame
    /// asks, and with ghostty's 463 listed a rebuild was most of the frame's
    /// derive. Same theme, no page; another, a page marking it.
    #[test]
    fn the_page_is_rebuilt_only_when_the_theme_worn_changes() {
        let mut list = list();
        let first = list.page("nord").expect("the first page");
        assert!(first.lines().iter().any(|l| l.text() == " ● nord"));
        assert_eq!(
            list.page("nord"),
            None,
            "the same theme: nothing to rebuild"
        );
        let next = list.page("mine").expect("another theme: a page");
        assert!(next.lines().iter().any(|l| l.text() == " ● mine"));
        assert!(next.lines().iter().any(|l| l.text() == "   nord"));
    }

    /// **One heading per place.** A file of yours named for a shipped theme
    /// keeps the shipped one's place in `Places::themes` — the name finds
    /// yours — and is listed with yours, not as a heading of its own in the
    /// middle of the shipped run.
    #[test]
    fn a_theme_of_yours_named_for_a_shipped_one_is_listed_with_yours() {
        let list = ThemeList::new(vec![
            ("terminal".into(), Source::Shipped),
            ("nord".into(), Source::Yours),
            ("dracula".into(), Source::Shipped),
            ("Sea".into(), Source::Imported("/b".into())),
            ("Air".into(), Source::Imported("/a".into())),
            ("mine".into(), Source::Yours),
            ("Bay".into(), Source::Imported("/b".into())),
        ]);
        let lines = list.lines("");
        let page: Vec<String> = lines.lines().iter().map(PageLine::text).collect();
        let names = named(&lines);
        let headings: Vec<&str> = page
            .iter()
            .enumerate()
            .filter(|&(i, l)| names[i].is_none() && !l.trim().is_empty() && i > 1)
            .map(|(_, l)| l.trim())
            .collect();
        assert_eq!(
            headings,
            [
                "shipped",
                "yours — themes/ beside your config",
                "imported — /b",
                "imported — /a"
            ],
            "{page:?}"
        );
        let at = |name: &str| {
            (names.iter())
                .position(|n| n.as_deref() == Some(name))
                .expect(name)
        };
        assert!(
            at("dracula") < at("nord") && at("nord") < at("mine"),
            "{page:?}"
        );
        assert!(at("Sea") < at("Bay") && at("Bay") < at("Air"), "{page:?}");
        assert_eq!(list.source_of("nord"), Some(&Source::Yours));
    }

    /// **The preview is drawn in the theme it shows**: its borders, its
    /// verdict and its fills, the terminal's colours — every one the
    /// candidate's own, never the theme around the pane.
    #[test]
    fn the_preview_wears_the_theme_it_shows() {
        let (theme, notes) = Theme::load("catppuccin-mocha", None).unwrap();
        let page = preview("catppuccin-mocha", "shipped", &theme, &notes, 100);
        let style_of = |needle: &str| {
            page.iter()
                .flat_map(|l| &l.runs)
                .find(|r| r.text.contains(needle))
                .map(|r| match r.look {
                    crate::paint::Look::Own(style) => style,
                    crate::paint::Look::Role(_) => panic!("{needle}: a role"),
                })
                .unwrap_or_else(|| panic!("{needle} is drawn"))
        };
        assert_eq!(style_of("┌ output — a reply"), theme.ui(Ink::BorderFocused));
        assert_eq!(style_of("rm build/old.txt"), theme.ui(Ink::DangerFill));
        assert_eq!(style_of("mkdir -p build"), theme.ui(Ink::CautionFill));
        assert_eq!(style_of("{{API_TOKEN}}"), theme.ui(Ink::Secret));
        // Its legend's keys as the decision pane paints them: a preview that
        // drew them plain would misstate the theme (review stop 17, C 7).
        let legend = crate::paint::hint("a allow once", CellStyle::PLAIN);
        let key = (legend.iter().find(|run| run.text == "a")).expect("premise: a key's run");
        let painted = crate::paint::Painter::new(&theme, Context::Decision).style(key);
        assert_ne!(painted, Style::default(), "premise: a key looks like one");
        let drawn = (page.iter().flat_map(|l| &l.runs))
            .find(|run| run.text == "a")
            .expect("the legend's key is drawn");
        assert_eq!(drawn.look, crate::paint::Look::Own(painted));
        // The tab line (Steven, 10-26): yours, another, one deciding.
        assert_eq!(style_of(" 1 auth-fix "), theme.ui(Ink::TabLineSelected));
        assert_eq!(style_of(" docs "), theme.ui(Ink::TabLine));
        assert_eq!(style_of(" 3 1a2b3c4d "), theme.ui(Ink::Caution));
        assert_ne!(
            theme.ui(Ink::TabLineSelected),
            theme.ui(Ink::TabLine),
            "premise: two looks to tell apart"
        );
        // The frame around your message: the one run that is nothing but a
        // rule (every box border carries a corner).
        let rule = page
            .iter()
            .flat_map(|l| &l.runs)
            .find(|r| !r.text.is_empty() && r.text.chars().all(|c| c == '─'))
            .expect("your message's rule is drawn");
        assert_eq!(
            rule.look,
            crate::paint::Look::Own(theme.ui(Ink::BorderUser))
        );
        assert_ne!(
            style_of("┌ output — a reply"),
            Theme::terminal().ui(Ink::BorderFocused),
            "premise: not the shipped terminal's"
        );
        // Every line fits the width it was built for.
        for l in &page {
            assert!(l.cells() <= 100, "{}", l.text());
        }
        let narrow = preview("catppuccin-mocha", "shipped", &theme, &notes, 40);
        for l in &narrow {
            assert!(l.cells() <= 40, "{}", l.text());
        }
    }
}
