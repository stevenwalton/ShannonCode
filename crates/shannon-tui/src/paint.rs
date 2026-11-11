//! **A page's lines as roles, painted when drawn** — what a [`TextView`]
//! holds, and the one way any of it becomes colour.
//!
//! The decision pane's report used to be painted when it was BUILT: its
//! lines were ratatui `Line`s in the theme's colours, so `ToolPrompt` kept an
//! `Arc<Theme>` to paint a re-gate the same way, a theme change had to
//! `restyle` the open report, and `:analyze` kept the colours it opened
//! with. A run carries its ROLE instead, and the theme is asked at the draw,
//! as the transcript has always been (`view::base_style`). Steven, 10-06:
//! approved, from the TODO entry "Paint the decision pane at draw".
//!
//! **Two kinds of look, and the difference is the security rule.** A
//! [`Look::Role`] is the theme's to paint, in the pane's [`Context`]; a
//! [`Look::Own`] is the app's, fixed — the decision pane's verdict (a
//! warning's fill, a safe command's green, the headline) is never a
//! theme's to change, and is `Own` for exactly that reason. Which map a
//! pane reads is chosen ONCE, by its kind (`view::painter`), not at each
//! leaf.
//!
//! [`TextView`]: crate::text_view::TextView

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::style::{CellStyle, Ink};
use crate::theme::{Context, Theme};

/// What a run looks like: a role the theme paints, or the app's own style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// The theme's, in the context of the pane that draws it.
    Role(CellStyle),
    /// The app's: no theme reaches it.
    Own(Style),
}

/// A stretch of one look — and an underline drawn over it, in a role's
/// colour (the decision pane's carrier variable, `ui.analyze.carrier.N`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub look: Look,
    pub underline: Option<Ink>,
}

impl Run {
    /// A run in the app's own `style` — emphasis, never a colour: every
    /// colour is a role's (`Run::role`), so a theme reaches it.
    pub fn own(text: impl Into<String>, style: Style) -> Run {
        Run {
            text: text.into(),
            look: Look::Own(style),
            underline: None,
        }
    }

    /// A run the theme paints, as `cell` says — a role and its emphasis.
    pub fn role(text: impl Into<String>, cell: CellStyle) -> Run {
        Run {
            text: text.into(),
            look: Look::Role(cell),
            underline: None,
        }
    }
}

/// **One line of a page**: its runs, and a style for the whole line (what
/// `Line::styled` gives a line, which a span-only reading would drop).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageLine {
    pub runs: Vec<Run>,
    pub style: Style,
}

impl PageLine {
    /// Display cells — what wrapping measures.
    pub fn cells(&self) -> usize {
        self.runs.iter().map(|r| r.text.width()).sum()
    }

    /// The text, runs joined — what a yank copies.
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    /// **This line in at most `width` cells**, never wrapped: the run it
    /// ends in cut with `…` (`wrap::clip`, by cell), the runs past it
    /// dropped. For a row whose columns must stay on one line — `:marks`',
    /// whose fixed columns overflowed a narrow pane and wrapped each row in
    /// two (review stop 14, B F7).
    pub fn clip(self, width: usize) -> PageLine {
        let mut used = 0;
        let mut runs = Vec::new();
        for run in self.runs {
            let room = width.saturating_sub(used);
            if room == 0 {
                break;
            }
            let text = crate::wrap::clip(&run.text, room);
            used += crate::wrap::str_cells(&text);
            let cut = text != run.text;
            runs.push(Run { text, ..run });
            if cut {
                break;
            }
        }
        PageLine {
            runs,
            style: self.style,
        }
    }

    /// **This line's rows at `width` cells** — word-wrapped by
    /// `wrap::wrap_cells_word`, the transcript's own wrapper, each row its
    /// runs cut where the row is (looks, underlines and the line's style
    /// kept). The ONE wrap of a page: what `TextView` measures by
    /// ([`rows`](Self::rows)) and what the renderer draws, so the two cannot
    /// disagree. They did: rows were counted by hard cell wrap and drawn by
    /// ratatui's word wrap, which needs more, so the measure came up short —
    /// always on the unsafe side — and the decision pane was sized by it
    /// (TODO.md's CRITICAL "the ruler we scroll by is not the ruler we
    /// render by"; the manual at 38 columns counted 2450 rows and drew
    /// 2549).
    pub fn wrap(&self, width: usize) -> Vec<PageLine> {
        self.cut(&self.row_ranges(width))
    }

    /// How many rows this line takes at `width` cells — [`wrap`](Self::wrap)'s
    /// count, by the same function.
    pub fn rows(&self, width: usize) -> usize {
        self.row_ranges(width).len()
    }

    /// **Where each row is, at `width` cells**: `(first char, chars shown)`
    /// — the spaces a break drops lie between one row and the next. What
    /// [`wrap`](Self::wrap) cuts by, and `TextView` cuts only the rows on
    /// screen from (`cut`).
    pub fn row_ranges(&self, width: usize) -> Vec<(usize, usize)> {
        let chars: Vec<char> = self.text().chars().collect();
        let mut at = 0;
        crate::wrap::wrap_cells_word(&chars, width)
            .iter()
            .map(|row| {
                let range = (at, row.text.chars().count());
                at += row.char_len;
                range
            })
            .collect()
    }

    /// **The rows `ranges` name** (from [`row_ranges`](Self::row_ranges)),
    /// their runs cut where the rows are — looks, underlines and the line's
    /// style kept. One pass: each char tagged with its run once, every row
    /// cut from that. It sliced each row from its run's start, so an
    /// unbroken line cost its length per row: 64K characters, half a second
    /// a frame (review 28, Reviewer C), in a command the model writes.
    pub fn cut(&self, ranges: &[(usize, usize)]) -> Vec<PageLine> {
        let tagged: Vec<(char, usize)> = self
            .runs
            .iter()
            .enumerate()
            .flat_map(|(i, run)| run.text.chars().map(move |c| (c, i)))
            .collect();
        ranges
            .iter()
            .map(|&(first, shown)| {
                let mut runs: Vec<Run> = Vec::new();
                let mut from = usize::MAX;
                for &(c, i) in tagged.get(first..first + shown).unwrap_or_default() {
                    match runs.last_mut() {
                        Some(run) if from == i => run.text.push(c),
                        _ => {
                            from = i;
                            runs.push(Run {
                                text: c.to_string(),
                                look: self.runs[i].look,
                                underline: self.runs[i].underline,
                            });
                        }
                    }
                }
                PageLine {
                    runs,
                    style: self.style,
                }
            })
            .collect()
    }

    /// **A line whose every character has a role** — a record's lines
    /// (`tool_record::Record::into_parts`), equal neighbours folded into one
    /// run. A style missing for a character is plain.
    pub fn of_cells(text: &str, styles: &[CellStyle]) -> PageLine {
        let mut runs: Vec<Run> = Vec::new();
        for (i, c) in text.chars().enumerate() {
            let look = Look::Role(styles.get(i).copied().unwrap_or(CellStyle::PLAIN));
            match runs.last_mut() {
                Some(run) if run.look == look => run.text.push(c),
                _ => runs.push(Run {
                    text: c.to_string(),
                    look,
                    underline: None,
                }),
            }
        }
        PageLine {
            runs,
            style: Style::default(),
        }
    }
}

/// **A row of keys and what each does** — `d/q deny · a allow once`, the
/// setup screen's `j/k move · Enter or 1–3 choose · Esc leave` — each item
/// [`hint_item`]'s, `·` between them in `words`. One painter for every such
/// row, so a key reads as a key in every pane: the decision's legend drew
/// `d/q` in the colour of `deny` (Steven, 11-08: the pane "should highlight
/// the commands").
pub(crate) fn hint(items: &str, words: CellStyle) -> Vec<Run> {
    runs_of(key_row(items), words)
}

/// [`hint`] of one item, for a row that joins its own.
pub(crate) fn hint_item(item: &str, words: CellStyle) -> Vec<Run> {
    runs_of(key_item(item), words)
}

/// A row of keys on the page's ground: the keys what you type (the manual's
/// `ui.help.code`), the rest in `words`.
fn runs_of(parts: Vec<(Part, String)>, words: CellStyle) -> Vec<Run> {
    let keys = CellStyle::ink(Ink::HelpCode);
    (parts.into_iter())
        .map(|(part, text)| {
            let look = match part {
                Part::Key => keys,
                Part::Words => words,
            };
            Run::role(text, look)
        })
        .collect()
}

/// **What a stretch of a row of keys is** — a caller on a ground of its own
/// (a filled bar, the status row) draws each part its own way, and asks this
/// rather than a run's colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    Key,
    Words,
}

/// **A row of keys, read**: the one reading [`hint`] paints and a caller on
/// its own ground draws.
pub(crate) fn key_row(items: &str) -> Vec<(Part, String)> {
    let mut parts = Vec::new();
    for (i, item) in items.split(" · ").enumerate() {
        if i > 0 {
            parts.push((Part::Words, " · ".to_string()));
        }
        parts.extend(key_item(item));
    }
    parts
}

/// **One item of a row of keys**: its keys — the first word, and any joined
/// to it by `or` or a comma (`Enter or 1–3`, `Enter, h`) — and the rest.
fn key_item(item: &str) -> Vec<(Part, String)> {
    let mut parts = Vec::new();
    let mut rest = item;
    loop {
        let end = rest.find(' ').unwrap_or(rest.len());
        let (key, comma) = match rest[..end].strip_suffix(',') {
            Some(key) => (key, true),
            None => (&rest[..end], false),
        };
        if !key.is_empty() {
            parts.push((Part::Key, key.to_string()));
        }
        rest = &rest[end..];
        let joined = match comma {
            true => rest.strip_prefix(' ').map(|next| (", ", next)),
            false => rest.strip_prefix(" or ").map(|next| (" or ", next)),
        };
        match joined.filter(|(_, next)| !next.is_empty()) {
            Some((join, next)) => {
                parts.push((Part::Words, join.to_string()));
                rest = next;
            }
            None => {
                if comma {
                    parts.push((Part::Words, ",".to_string()));
                }
                break;
            }
        }
    }
    if !rest.is_empty() {
        parts.push((Part::Words, rest.to_string()));
    }
    parts
}

/// A line built in the app's own colours is a page line of `Own` runs,
/// unchanged when painted: the holders that pick no theme colour (`:help`,
/// `:messages`, the listings, the inspector) build `Line`s as they did.
impl From<Line<'static>> for PageLine {
    fn from(line: Line<'static>) -> PageLine {
        PageLine {
            runs: line
                .spans
                .into_iter()
                .map(|span| Run::own(span.content.into_owned(), span.style))
                .collect(),
            style: line.style,
        }
    }
}

/// **A theme and a context** — how one pane turns roles into colours.
/// Chosen once per pane (`view::painter`); every run on it is painted by it.
#[derive(Debug, Clone, Copy)]
pub struct Painter<'t> {
    theme: &'t Theme,
    context: Context,
}

impl<'t> Painter<'t> {
    pub fn new(theme: &'t Theme, context: Context) -> Painter<'t> {
        Painter { theme, context }
    }

    /// The theme it paints with — for the pane's own chrome, its border.
    pub fn theme(&self) -> &'t Theme {
        self.theme
    }

    /// A run's style: its look, then its underline over it.
    pub fn style(&self, run: &Run) -> Style {
        let base = match run.look {
            Look::Role(cell) => self.theme.style(self.context, cell),
            Look::Own(style) => style,
        };
        match run.underline {
            Some(role) => base
                .add_modifier(Modifier::UNDERLINED)
                .underline_color(self.theme.fg(role)),
            None => base,
        }
    }

    /// A page line, painted.
    pub fn line(&self, line: &PageLine) -> Line<'static> {
        Line::from(
            line.runs
                .iter()
                .map(|run| match self.style(run) {
                    style if style == Style::default() => Span::raw(run.text.clone()),
                    style => Span::styled(run.text.clone(), style),
                })
                .collect::<Vec<_>>(),
        )
        .style(line.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    /// **A row of keys draws its keys as keys** — each item's first word,
    /// and those joined to it by `or` or a comma; the words, the `·` and
    /// the joins in the row's own look. An item that is one sentence keeps
    /// its words.
    #[test]
    fn a_row_of_keys_draws_its_keys_as_keys() {
        let key = Look::Role(CellStyle::ink(Ink::HelpCode));
        let read = |items: &str| -> Vec<(String, bool)> {
            (hint(items, CellStyle::PLAIN).into_iter())
                .map(|run| (run.text, run.look == key))
                .collect()
        };
        let want = |runs: &[(&str, bool)]| -> Vec<(String, bool)> {
            runs.iter()
                .map(|(text, key)| (text.to_string(), *key))
                .collect()
        };
        assert_eq!(
            read("d/q deny · a allow once"),
            want(&[
                ("d/q", true),
                (" deny", false),
                (" · ", false),
                ("a", true),
                (" allow once", false)
            ])
        );
        assert_eq!(
            read("Enter or 1–3 choose"),
            want(&[
                ("Enter", true),
                (" or ", false),
                ("1–3", true),
                (" choose", false)
            ])
        );
        assert_eq!(
            read("Enter, h shut"),
            want(&[
                ("Enter", true),
                (", ", false),
                ("h", true),
                (" shut", false)
            ])
        );
        assert_eq!(
            read("y removes it, any other key keeps it"),
            want(&[("y", true), (" removes it, any other key keeps it", false)])
        );
        assert_eq!(read(""), want(&[]));
    }

    /// **A line built in the app's colours paints back as itself** — every
    /// span's style, and the line's own — whatever the theme: the holders
    /// that convert (`From<Line>`) must draw exactly as they did.
    #[test]
    fn an_own_line_paints_back_unchanged() {
        let line = Line::from(vec![
            Span::raw("plain "),
            Span::styled("red", Style::default().fg(Color::Red)),
        ])
        .style(Style::default().add_modifier(Modifier::DIM));
        let page = PageLine::from(line.clone());
        assert_eq!(page.text(), "plain red");
        assert_eq!(page.cells(), 9);
        for context in [Context::Code, Context::Decision] {
            assert_eq!(Painter::new(&Theme::terminal(), context).line(&page), line);
        }
    }

    /// **A line clipped to a width ends at its cut** — by cells, a wide
    /// character never split, the run it ends in said `…` and nothing after
    /// it, each run's look kept; a line that fits untouched.
    #[test]
    fn a_clipped_line_ends_at_its_cut() {
        let quiet = CellStyle::ink(Ink::TextFaint);
        let line = PageLine {
            runs: vec![
                Run::role("ab", CellStyle::PLAIN),
                Run::role("日本語", quiet),
                Run::role("cd", CellStyle::PLAIN),
            ],
            ..PageLine::default()
        };
        assert_eq!(line.clone().clip(10), line, "fits: untouched");
        // Six cells: `本` would take the fifth and sixth, so the cut stops a
        // cell short — and that cell stays empty, no run after the cut.
        let cut = line.clone().clip(6);
        assert_eq!(cut.text(), "ab日…", "{cut:?}");
        assert!(cut.cells() <= 6);
        assert_eq!(cut.runs[1].look, Look::Role(quiet), "its look kept");
        assert_eq!(line.clip(0).text(), "", "no room, nothing");
    }

    /// **A role is the theme's, in the pane's context** — and an underline is
    /// drawn over whatever the look gave.
    #[test]
    fn a_role_is_painted_by_the_panes_map() {
        let theme = Theme::terminal();
        let string = CellStyle {
            ink: Some(Ink::String),
            ..CellStyle::PLAIN
        };
        let run = Run {
            text: "'x'".into(),
            look: Look::Role(string),
            underline: Some(Ink::Carrier2),
        };
        for context in [Context::Code, Context::Decision] {
            let style = Painter::new(&theme, context).style(&run);
            assert_eq!(style.fg, theme.style(context, string).fg, "{context:?}");
            assert!(style.add_modifier.contains(Modifier::UNDERLINED));
            assert_eq!(style.underline_color, Some(Color::Magenta), "the carrier's");
        }
        assert_ne!(
            theme.style(Context::Code, string).fg,
            theme.style(Context::Decision, string).fg,
            "premise: the maps differ"
        );
    }
}
