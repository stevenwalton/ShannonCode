//! **What a decision pane shows** — a body that scrolls, and the keys that
//! answer it pinned beneath (`docs/DesignDocs/decision-panes-and-alerts.md`
//! §1, §4). Two: a call's report (`permission_view::ToolPrompt`) and a
//! summary's draft waiting for its review (`app::review::Review`), each drawn
//! by `view::draw_decision` and moved through by `Decisions::page_mut`.
//!
//! The rules that keep a decision's keys and the end of its body on screen —
//! the legend pinned as a footer, the fold marker on a row of its own, the
//! box sized in RENDERED rows — were each found on the call, at a cost (a
//! report whose danger sat below the fold of a box that looked complete).
//! Stated here once, as the trait's own methods, a second kind of decision
//! keeps them by being one, rather than by copying them.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Modifier, Style};

use crate::paint::PageLine;
use crate::text_view::TextView;

/// The smallest body, in `(rows, columns)`, that can spare a row for the
/// fold marker. Below it the marker is not drawn and no row is reserved.
pub(crate) const FOLD_MIN: (u16, u16) = (2, 8);

/// **One row of a decision's keys** — what pressing it does, and how each
/// place that lists keys says it ([`KeyTable`]).
pub(crate) struct KeyRow<A> {
    /// The plain keys that do it — none for a row that is a command
    /// (`:hide later`).
    pub(crate) codes: Vec<KeyCode>,
    /// What it answers; `None` for one that answers nothing — scrolling,
    /// `n`/`N` walking the calls waiting — listed so you know it is there.
    pub(crate) action: Option<A>,
    /// What the legend and the not-a-key text call it: `d/q deny`.
    pub(crate) label: String,
    /// What the status row calls it when the pane's legend is cut
    /// (`d deny`); `None`: left out there — every row that answers nothing,
    /// and an answer too long for a row (`A`'s rule).
    pub(crate) short: Option<&'static str>,
    /// **Struck through**: the action exists and cannot apply to this one (a
    /// Claude Code call whose edit does not map back) — said, not dropped
    /// (Steven, 10-04: it says the action exists and is unavailable HERE,
    /// which a missing key cannot); its key still resolves, to say why.
    pub(crate) struck: bool,
    /// **Listed** — `false` for a key that answers but is not offered: `A`
    /// with no rule to propose resolves, to say why there is none, and is
    /// listed only with a rule (Reviewer F, round 18: listing it always
    /// pointed at a key that could only say no rule would help).
    pub(crate) listed: bool,
}

impl<A> KeyRow<A> {
    /// A listed row, not struck.
    pub(crate) fn new(
        codes: &[KeyCode],
        action: Option<A>,
        label: impl Into<String>,
        short: Option<&'static str>,
    ) -> Self {
        KeyRow {
            codes: codes.to_vec(),
            action,
            label: label.into(),
            short,
            struck: false,
            listed: true,
        }
    }
}

/// **A decision's keys, stated once** (review stop 8, C 8 / D 4): what
/// [`resolve`](Self::resolve) answers, what the [`legend`](Self::legend)
/// lists, what the status row says when the legend is cut
/// ([`in_short`](Self::in_short)) and what a refused key is told
/// ([`not_a_key`](Self::not_a_key)) are all read off one table per kind,
/// built from what the decision is now. They were up to eight statements
/// per kind, derived from none, and disagreed: a denied call's short form
/// offered `a allow`, a dead key; its legend left out `j/k scroll` while
/// its refusal listed it. Ordered as the legend reads: the answers first,
/// so a narrow pane cuts navigation, never an answer (review stop 8, F 3).
pub(crate) struct KeyTable<A> {
    rows: Vec<KeyRow<A>>,
}

impl<A: Clone> KeyTable<A> {
    pub(crate) fn new(rows: Vec<KeyRow<A>>) -> Self {
        KeyTable { rows }
    }

    /// **What `key` answers** — the whole key, plain only
    /// (`keymap::is_plain`): the one answer the pane's answer path and its
    /// no-room guard both read, so `<C-a>` answers neither (it was refused as
    /// one by a guard asking of the character alone; DR-B #8). `None` for a
    /// key the table has no answer for.
    pub(crate) fn resolve(&self, key: &KeyEvent) -> Option<A> {
        if !crate::keymap::is_plain(key) {
            return None;
        }
        (self.rows.iter())
            .find(|row| row.codes.contains(&key.code))
            .and_then(|row| row.action.clone())
    }

    /// The rows offered, in order.
    fn listed(&self) -> impl Iterator<Item = &KeyRow<A>> {
        self.rows.iter().filter(|row| row.listed)
    }

    /// **The pinned legend**: `answer:` and every row offered, its keys
    /// drawn as keys (`paint::hint_item`, every row of keys' painter), a
    /// struck one struck through whole. Named, not pointed at: a `→` before
    /// a list of options is a selection cursor in every TUI that has one,
    /// and nothing here is selected — each key commits (the lead-in's
    /// history is `permission_view::analyze_line`'s).
    pub(crate) fn legend(&self) -> PageLine {
        use crate::paint::Run;
        use crate::style::CellStyle;
        let struck = Style::default().add_modifier(Modifier::CROSSED_OUT | Modifier::DIM);
        let mut runs = vec![Run::role(
            "  answer: ",
            CellStyle::ink(crate::style::Ink::Tool),
        )];
        for (i, row) in self.listed().enumerate() {
            if i > 0 {
                runs.push(Run::role(" · ", CellStyle::PLAIN));
            }
            match row.struck {
                true => runs.push(Run::own(row.label.clone(), struck)),
                false => runs.extend(crate::paint::hint_item(&row.label, CellStyle::PLAIN)),
            }
        }
        PageLine {
            runs,
            style: Style::default(),
        }
    }

    /// **Its answers in short** — every row offered, not struck, with a
    /// short word (only answers have one: a row's `short` says so): what a
    /// status row standing in for a cut legend offers (`$decide`). An answer
    /// the call does not have is not in the table, so it cannot be here: a
    /// denied call offered `a allow` (D 4).
    pub(crate) fn in_short(&self) -> String {
        (self.listed())
            .filter(|row| !row.struck)
            .filter_map(|row| row.short)
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// **How wide the legend must be to show every answer you can give** —
    /// in cells, from its `answer:` to the end of the last row the short
    /// form holds ([`in_short`](Self::in_short)). The status row takes the
    /// keys from a legend narrower than this (review stop 8, F 3): the
    /// review's `d drop` was cut in the `:graph` layout `S` is pressed from,
    /// and the row, seeing a legend, said nothing. Not the whole legend's
    /// width: a call's is cut only in its navigation tail and `A`'s rule —
    /// which it was ordered for — and asking that put `DECIDE:` on a call's
    /// row with every answer drawn.
    pub(crate) fn answers_width(&self) -> usize {
        use unicode_width::UnicodeWidthStr as _;
        let mut width = "  answer: ".width();
        let mut answers = width;
        for (i, row) in self.listed().enumerate() {
            width += if i > 0 { " · ".width() } else { 0 } + row.label.width();
            if row.short.is_some() && !row.struck {
                answers = width;
            }
        }
        answers
    }

    /// **What a key this pane does not take is told** — the rows offered, as
    /// the legend lists them.
    pub(crate) fn not_a_key(&self) -> String {
        let keys: Vec<&str> = self.listed().map(|row| row.label.as_str()).collect();
        format!("not a decision key — {}", keys.join(" · "))
    }
}

/// **A page a decision pane draws**: its body, the legend that answers it,
/// its title and its keys. Its sizing is the trait's, so every kind sizes
/// alike; what differs between kinds — the typed answer and where it goes —
/// stays with `OnPane` and `Answer` (review stop 8, C 13).
pub(crate) trait DecisionPage {
    /// The scrollable body.
    fn text(&self) -> &TextView;

    /// The body, to move through — the keymap's motions, and the clamp that
    /// sizes it to its pane.
    fn text_mut(&mut self) -> &mut TextView;

    /// The pinned legend — the keys that answer it — rendered as a footer
    /// below the body, never scrolled with it.
    fn legend(&self) -> Option<&PageLine>;

    /// **The pane's title**, for a body `rows` tall and `width` wide — the
    /// kind's own: a call's carries its command only when the body cannot
    /// show it.
    fn title(&self, rows: u16, width: u16) -> String;

    /// **What it asks about, on one row** — where the pane cannot show it
    /// (the row's `$decide`).
    fn subject(&self) -> String;

    /// Its answers in short ([`KeyTable::in_short`]).
    fn keys_in_short(&self) -> String;

    /// What a key it does not take is told ([`KeyTable::not_a_key`]).
    fn not_a_key(&self) -> String;

    /// How wide its legend must be to show its answers
    /// ([`KeyTable::answers_width`]).
    fn answers_width(&self) -> usize;

    /// **A held value drawn in it is one the model wrote itself** — Claude
    /// Code's line: the gate reads handles only, so nothing else says the
    /// line carries your secret (review stop 8, E2 5).
    fn leaked(&self) -> bool {
        false
    }

    /// **The rows the body scrolls in**, inside a pane `height` rows tall
    /// and `width` wide: less the pinned legend, and less the fold marker's
    /// row when the body does not fit.
    ///
    /// The draw lays the pane out by this, and every clamp of the body's
    /// `TextView` sizes it by this. They disagreed: the clamps used the whole
    /// content height, so the view believed it showed the two rows the legend
    /// and the marker cover. On a capped report, `G` put the cursor — and the
    /// report's last two lines, the per-stage detail — under the legend, with
    /// "↓ 2 more" drawn above it and no key that could reach them.
    ///
    /// **Independent of the scroll, on purpose.** The marker moves to the top
    /// once you are scrolled, but whether a row is reserved depends only on
    /// whether the body fits. Were it scroll-dependent, a `zt` would change
    /// the height, every clamp would read that as a resize, and the resize
    /// rule would undo the `zt`.
    fn body_rows(&self, height: u16, width: u16) -> u16 {
        let legend = u16::from(self.legend().is_some() && height > 1);
        let avail = height - legend;
        let fold = u16::from(
            avail >= FOLD_MIN.0
                && width >= FOLD_MIN.1
                && self.text().rows_at(width) > avail as usize,
        );
        avail - fold
    }

    /// How many rows the pane should size itself to at `width` (before the
    /// caller applies its maximum) — the body plus the pinned footer.
    /// **Content drives the box**: a short body gets a short box, and there
    /// is then nothing to scroll.
    ///
    /// **Measured in RENDERED rows, not logical lines**, and that distinction
    /// is a security property rather than a cosmetic one. This counted
    /// `text.lines().len()` while the pane renders with `Wrap { trim: false }`
    /// — the same number only while nothing wraps. The **model controls the
    /// command string**, therefore it controls the wrap ratio, therefore it
    /// controlled how much of its own danger report sat below the fold of a
    /// box that drew its border as though complete: a realistic 386-cell
    /// `grep` line needed 13 rows in a 9-row body, hiding `[ask] rm ·
    /// destructive` and the sentence explaining why (Reviewer E, reproduced
    /// with numbers). The verdict headline survives at row 0, and the verdict
    /// headline says `ASK` for a huge class of ordinary commands — what falls
    /// off is *which stage is dangerous*.
    ///
    /// `TextView::rows_at` is the right unit and already existed; every
    /// *scroll* consumer used it and only this *sizing* consumer counted for
    /// itself (`[[project_shannon_two_predicates]]` — "is it on screen", "can it
    /// be answered" and "how big is the box" are one question).
    fn content_rows(&self, width: u16) -> u16 {
        let footer = usize::from(self.legend().is_some());
        (self.text().rows_at(width) + footer).min(u16::MAX as usize) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    /// **The legend draws its keys as keys** (Steven, 11-08: the pane
    /// "should highlight the commands") — drawn as the decision pane draws
    /// (`view::painter`'s context for it): `d/q` in the colour of what you
    /// type, `deny` not; a struck row struck through whole, its key too.
    #[test]
    fn the_legend_draws_its_keys_as_keys() {
        use crate::style::{CellStyle, Ink};
        let table = KeyTable::new(vec![
            KeyRow::new(&[KeyCode::Char('d')], Some(1), "d/q deny", Some("d deny")),
            KeyRow {
                struck: true,
                ..KeyRow::new(&[KeyCode::Char('e')], Some(2), "e edit", Some("e edit"))
            },
        ]);
        let theme = crate::theme::Theme::terminal();
        let paint = crate::view::painter(crate::workspace::PaneKind::Tool, &theme);
        let line = paint.line(&table.legend());
        let style = |text: &str| {
            (line.spans.iter())
                .find(|s| s.content == text)
                .unwrap_or_else(|| panic!("{text:?} in {line:?}"))
                .style
        };
        let code = paint
            .style(&crate::paint::Run::role("x", CellStyle::ink(Ink::HelpCode)))
            .fg;
        assert!(code.is_some(), "premise: what you type has a colour");
        assert_eq!(style("d/q").fg, code, "the keys");
        assert_ne!(style(" deny").fg, code, "the words");
        let struck = style("e edit");
        assert!(
            struck.add_modifier.contains(Modifier::CROSSED_OUT),
            "{struck:?}"
        );
        assert_ne!(
            struck.fg, code,
            "a key it cannot apply is not offered as one"
        );
    }

    /// **One table, read four ways** (review stop 8, C 8): what the legend
    /// lists is what the refusal lists; the short form holds only answers
    /// you can give — not a row that answers nothing, not one struck through,
    /// not one unlisted; an unlisted row still resolves (the designed `A`
    /// with no rule), and a modified key answers nothing.
    #[test]
    fn a_key_table_is_read_the_same_four_ways() {
        let key = |c, m| KeyEvent::new(KeyCode::Char(c), m);
        let table = KeyTable::new(vec![
            KeyRow::new(&[KeyCode::Char('d')], Some(1), "d deny", Some("d deny")),
            KeyRow {
                struck: true,
                ..KeyRow::new(&[KeyCode::Char('e')], Some(2), "e edit", Some("e edit"))
            },
            KeyRow::new(&[KeyCode::Char('j')], None, "j/k scroll", None),
            KeyRow {
                listed: false,
                ..KeyRow::new(&[KeyCode::Char('A')], Some(3), "A always", Some("A"))
            },
        ]);
        let legend = table.legend().text();
        assert_eq!(legend, "  answer: d deny · e edit · j/k scroll");
        assert_eq!(
            table.not_a_key(),
            "not a decision key — d deny · e edit · j/k scroll"
        );
        assert_eq!(table.in_short(), "d deny", "answers you can give, only");
        assert_eq!(
            table.answers_width(),
            "  answer: d deny".len(),
            "the legend shows its answers once it reaches the last one you can give"
        );
        assert_eq!(table.resolve(&key('d', KeyModifiers::NONE)), Some(1));
        assert_eq!(
            table.resolve(&key('e', KeyModifiers::NONE)),
            Some(2),
            "struck, still says why"
        );
        assert_eq!(
            table.resolve(&key('A', KeyModifiers::SHIFT)),
            Some(3),
            "unlisted, still resolves"
        );
        assert_eq!(
            table.resolve(&key('j', KeyModifiers::NONE)),
            None,
            "answers nothing"
        );
        assert_eq!(table.resolve(&key('x', KeyModifiers::NONE)), None);
        assert_eq!(
            table.resolve(&key('d', KeyModifiers::CONTROL)),
            None,
            "plain only"
        );
    }
}
