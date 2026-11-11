//! **Themes** — what each highlight role ([`Ink`]) looks like (Steven, 10-06:
//! "I'm showing you the theme not to replicate the exact colors, but to show
//! you the mappings. If we can inherit colors in a similar way that would be
//! nice"). `docs/DesignDocs/themes.md` is the design.
//!
//! **A theme maps roles, never text.** Every highlighter — the gate's shell
//! lexer, syntect, markdown, and next a language server — says what a piece
//! of text IS; the theme alone says what that looks like. So one theme
//! paints a command in the transcript, a model's code block and a file a
//! `Read` returned alike, and a new one recolours the transcript without
//! highlighting it again (the cache holds roles, `style::CellStyle`).
//!
//! **The shipped theme names terminal colours, not values** (`TERMINAL`):
//! `keyword` is the terminal's magenta, whatever magenta the terminal's own
//! theme makes it — as `bat --theme=ansi` does — so Shannon wears ghostty's
//! colours, or anyone's, without a theme of its own. A theme file may name
//! 24-bit colours instead.
//!
//! **Two contexts** ([`Context`]): the decision pane is the one place green,
//! yellow and red are a VERDICT, so a string painted green there would read
//! as "this is safe". A theme's `decision.*` entries paint the pane; any
//! role they do not name falls back to its code entry. The shipped ones keep
//! the shell off the verdict's hues, and a theme file that puts them there
//! is told so at load.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ratatui::style::{Color, Modifier, Style};

use crate::palette;
use crate::style::{CellStyle, Ink};

mod ghostty;

/// **The directories `tui.theme_import` names**: `ghostty`, ghostty's own
/// (`ghostty::dirs`); any other entry a directory of terminal palettes, `~/`
/// your home. `env` is the environment — a parameter, so a test reads none
/// of this machine's.
pub fn import_dirs(entries: &[String], env: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let home = env("HOME").filter(|h| !h.is_empty());
    entries
        .iter()
        .flat_map(|entry| match entry.as_str() {
            "ghostty" => ghostty::dirs(env),
            path => match (path.strip_prefix("~/"), &home) {
                (Some(rest), Some(home)) => vec![Path::new(home).join(rest)],
                _ => vec![PathBuf::from(path)],
            },
        })
        .collect()
}

/// How many roles there are — the length of a resolved theme's tables.
const ROLES: usize = Ink::ALL.len();

/// Where text is drawn, which decides which of a theme's maps paints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// Everywhere but the gate: the transcript, a record, a model's code.
    Code,
    /// The decision pane, where green, yellow and red are the verdict's.
    Decision,
}

/// What a role looks like: a colour and a background (`None`, the
/// terminal's own) and emphasis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Paint {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
}

impl Paint {
    const fn fg(colour: Color) -> Paint {
        Paint {
            fg: Some(colour),
            bg: None,
            bold: false,
            italic: false,
            dim: false,
        }
    }

    /// A background alone.
    const fn ground(colour: Color) -> Paint {
        Paint {
            fg: None,
            bg: Some(colour),
            bold: false,
            italic: false,
            dim: false,
        }
    }

    /// This paint on `colour`.
    const fn on(mut self, colour: Color) -> Paint {
        self.bg = Some(colour);
        self
    }

    const fn bold(mut self) -> Paint {
        self.bold = true;
        self
    }

    const fn italic(mut self) -> Paint {
        self.italic = true;
        self
    }
}

/// **The shipped theme**: terminal colours by name, so the terminal's palette
/// decides the values. Code roles follow the convention most terminal
/// themes are drawn for (keywords magenta, strings green, functions blue,
/// types yellow, comments the dim grey); the decision pane's shell roles
/// keep off green, yellow and red.
const TERMINAL: &[(&str, Paint)] = &[
    ("ui.tool", Paint::fg(palette::TOOL)),
    ("ui.muted", Paint::fg(palette::MUTED)),
    ("diff.plus", Paint::fg(palette::SAFE)),
    ("diff.minus", Paint::fg(palette::DANGER)),
    ("comment", Paint::fg(Color::DarkGray).italic()),
    ("keyword", Paint::fg(Color::Magenta)),
    ("string", Paint::fg(Color::Green)),
    ("string.special", Paint::fg(Color::Cyan)),
    ("number", Paint::fg(Color::LightMagenta)),
    ("constant", Paint::fg(Color::LightMagenta)),
    ("function", Paint::fg(Color::Blue)),
    // A command's name: what runs, so it stands out (the shell lexer gives
    // it; `permission_view::COMMAND` carries the role, not the weight).
    ("function.call", Paint::fg(Color::Blue).bold()),
    ("function.builtin", Paint::fg(Color::Cyan)),
    ("function.macro", Paint::fg(Color::Cyan)),
    ("type", Paint::fg(Color::Yellow)),
    ("variable.parameter", Paint::fg(Color::LightBlue)),
    ("variable.special", Paint::fg(Color::Cyan)),
    ("attribute", Paint::fg(Color::Cyan)),
    ("tag", Paint::fg(Color::Red)),
    ("label", Paint::fg(Color::Blue)),
    ("markup.heading", Paint::fg(Color::Blue).bold()),
    ("markup.raw", Paint::fg(Color::Yellow)),
    ("markup.raw.fence", Paint::fg(Color::DarkGray)),
    ("diagnostic.error", Paint::fg(Color::Red)),
    ("diagnostic.warning", Paint::fg(Color::Yellow)),
    ("diagnostic.info", Paint::fg(Color::Blue)),
    ("diagnostic.hint", Paint::fg(Color::Cyan)),
    // The decision pane: the shell, off the verdict's hues.
    ("decision.function.call", Paint::fg(Color::White).bold()),
    ("decision.variable.parameter", Paint::fg(Color::LightBlue)),
    ("decision.string", Paint::fg(Color::LightCyan)),
    ("decision.variable.special", Paint::fg(Color::LightMagenta)),
    ("decision.operator", Paint::fg(Color::Gray)),
    ("decision.keyword", Paint::fg(Color::Blue)),
    // …and a Write's diff, in its file's syntax: off them too.
    ("decision.type", Paint::fg(Color::Cyan)),
    ("decision.tag", Paint::fg(Color::LightMagenta)),
    ("decision.markup.raw", Paint::fg(Color::LightCyan)),
    // The UI's own roles, as `palette.rs` names them. `ui.background` is
    // not here: unset, every pane keeps the terminal's ground.
    ("ui.text", Paint::fg(palette::TEXT)),
    ("ui.text.quiet", Paint::fg(palette::MUTED)),
    ("ui.text.faint", Paint::fg(palette::FAINT)),
    // `ui.text.quiet.thinking` is not here: unset, it is quiet text, as
    // each theme paints that (`Ink::Thinking`).
    ("ui.nontext", Paint::fg(palette::NON_TEXT)),
    ("ui.statusline", Paint::fg(palette::TEXT)),
    // The tab line: the others quiet, yours as a chosen item is.
    ("ui.tabline", Paint::fg(palette::MUTED)),
    (
        "ui.tabline.selected",
        Paint::fg(palette::INVERSE_FG)
            .on(palette::INVERSE_BG)
            .bold(),
    ),
    ("ui.border", Paint::fg(palette::CHROME)),
    ("ui.border.focused", Paint::fg(palette::FOCUSED)),
    ("ui.border.aimed", Paint::fg(palette::AIMED)),
    (
        "ui.menu.selected",
        Paint::fg(palette::INVERSE_FG).on(palette::INVERSE_BG),
    ),
    ("ui.chosen", Paint::fg(palette::FOCUSED).bold()),
    // The bars dark on their hue, as `ui.caution.fill` is and every named
    // theme's base on cyan: the terminal's white on its cyan or yellow is
    // two light colours in most palettes (Steven, 11-09, image 47: "The
    // white on cyan is a bit hard to read").
    (
        "ui.bar",
        Paint::fg(palette::INVERSE_FG).on(palette::FOCUSED).bold(),
    ),
    (
        "ui.bar.waiting",
        Paint::fg(palette::INVERSE_FG).on(palette::CAUTION).bold(),
    ),
    ("ui.safe", Paint::fg(palette::SAFE)),
    ("ui.caution", Paint::fg(palette::CAUTION)),
    ("ui.danger", Paint::fg(palette::DANGER)),
    (
        "ui.danger.sign",
        Paint::fg(palette::INVERSE_FG).on(palette::DANGER).bold(),
    ),
    (
        "ui.caution.fill",
        Paint::fg(palette::INVERSE_FG).on(palette::CAUTION),
    ),
    (
        "ui.danger.fill",
        Paint::fg(palette::TEXT).on(palette::DANGER).bold(),
    ),
    ("ui.question", Paint::fg(palette::QUESTION)),
    ("ui.operand", Paint::fg(palette::OPERAND)),
    ("ui.secret", Paint::fg(palette::SECRET).bold()),
    // An alarm, unlike `ui.secret`: the model had a value it never should.
    ("ui.secret.leaked", Paint::fg(palette::DANGER).bold()),
    ("ui.graph.tree", Paint::fg(palette::TREE)),
    ("ui.graph.running", Paint::fg(palette::RUNNING)),
    ("ui.graph.held", Paint::fg(palette::HELD)),
    ("ui.graph.shared", Paint::fg(palette::SHARED_ROW).italic()),
    ("ui.graph.mark", Paint::ground(palette::MARK_BG)),
    ("ui.graph.continued", Paint::fg(palette::CONTINUED)),
    ("ui.graph.lane.1", Paint::fg(palette::LANES[0])),
    ("ui.graph.lane.2", Paint::fg(palette::LANES[1])),
    ("ui.graph.lane.3", Paint::fg(palette::LANES[2])),
    ("ui.graph.lane.4", Paint::fg(palette::LANES[3])),
    ("ui.graph.lane.5", Paint::fg(palette::LANES[4])),
    ("ui.graph.lane.6", Paint::fg(palette::LANES[5])),
    ("ui.analyze.carrier.1", Paint::fg(palette::CARRIERS[0])),
    ("ui.analyze.carrier.2", Paint::fg(palette::CARRIERS[1])),
    ("ui.analyze.carrier.3", Paint::fg(palette::CARRIERS[2])),
    ("ui.analyze.carrier.4", Paint::fg(palette::CARRIERS[3])),
    ("ui.help.heading", Paint::fg(palette::HELP_HEADING)),
    ("ui.help.tag", Paint::fg(palette::HELP_TAG)),
    ("ui.help.ref", Paint::fg(palette::HELP_REF)),
    ("ui.help.code", Paint::fg(palette::HELP_CODE)),
    ("ui.listing.heading", Paint::fg(palette::LISTING_HEADING)),
];

/// The context prefix a theme writes a decision-pane entry under.
const DECISION: &str = "decision.";

/// **What a theme says the sixteen terminal colours are** — `[palette]`
/// entries named `black` … `bright-white`, by slot. `None` keeps the
/// terminal's own.
pub type Ansi = [Option<Color>; 16];

/// No terminal colour said: each is the terminal's.
const TERMINAL_ANSI: Ansi = [None; 16];

/// **A theme, resolved**: each role's paint in each context, looked up by
/// index on the render path, and what it says the terminal's colours are.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    name: String,
    code: [Paint; ROLES],
    decision: [Paint; ROLES],
    ansi: Ansi,
}

impl Default for Theme {
    fn default() -> Theme {
        Theme::terminal()
    }
}

impl Theme {
    /// The shipped theme's name: `tui.theme`'s default, by construction.
    pub const DEFAULT: &'static str = shannon_core::config::DEFAULT_THEME;

    /// The shipped theme (`TERMINAL`).
    pub fn terminal() -> Theme {
        Theme::resolve(Theme::DEFAULT, &entries(TERMINAL), &TERMINAL_ANSI)
    }

    /// Its name, as `tui.theme` gave it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What `ink` looks like in `context`.
    pub fn paint(&self, context: Context, ink: Ink) -> Paint {
        let table = match context {
            Context::Code => &self.code,
            Context::Decision => &self.decision,
        };
        table[ink as usize]
    }

    /// **A cached style, as this theme draws it in `context`** — the one
    /// place a role becomes a colour. The text's own emphasis (`**bold**`)
    /// adds to the role's.
    pub fn style(&self, context: Context, cell: CellStyle) -> Style {
        let paint = cell
            .ink
            .map(|ink| self.paint(context, ink))
            .unwrap_or_default();
        let mut style = Style::default();
        if let Some(colour) = paint.fg {
            style = style.fg(colour);
        }
        if let Some(colour) = paint.bg {
            style = style.bg(colour);
        }
        let mut mods = Modifier::empty();
        mods.set(Modifier::BOLD, cell.bold || paint.bold);
        mods.set(Modifier::ITALIC, cell.italic || paint.italic);
        mods.set(Modifier::DIM, cell.dim || paint.dim);
        style.add_modifier(mods)
    }

    /// **What a UI role looks like** (`ui.border.focused`, `ui.danger`) —
    /// its colour, background and emphasis. Every colour the UI draws is
    /// asked here or through a page's [`Painter`](crate::paint::Painter);
    /// `palette.rs` holds only the shipped theme's values.
    pub fn ui(&self, ink: Ink) -> Style {
        self.style(Context::Code, CellStyle::ink(ink))
    }

    /// A role's colour, for a site that composes a style of its own —
    /// `Reset`, the terminal's own, where the theme says none.
    pub fn fg(&self, ink: Ink) -> Color {
        self.paint(Context::Code, ink).fg.unwrap_or(Color::Reset)
    }

    /// **The terminal cursor's colour** — `ui.cursor`'s `bg`, or its bare
    /// colour — as `#rrggbb` is all a terminal is told it in (OSC 12);
    /// `None`, the terminal's own. Every pane places the terminal's cursor
    /// (`view::place`), so its colour was the terminal's config's, chosen
    /// for the terminal's ground: under a theme that paints its own, a light
    /// one on a dark terminal, it was a faint yellow on cream (Steven,
    /// 11-02, image 42), while a page's then-painted cell followed the theme.
    pub fn cursor(&self) -> Option<(u8, u8, u8)> {
        let paint = self.paint(Context::Code, Ink::Cursor);
        match paint.bg.or(paint.fg) {
            Some(Color::Rgb(r, g, b)) => Some((r, g, b)),
            _ => None,
        }
    }

    /// `colour` as this theme says it: a terminal colour it names, its
    /// value — so a colour written outside the theme (`tui.ctx_colors`'
    /// `70:yellow`) is the theme's yellow, as every role's is.
    pub fn said(&self, colour: Color) -> Color {
        said(colour, &self.ansi)
    }

    /// What this theme says terminal colour `slot` (0-15) is — the
    /// terminal's own colour where it says nothing.
    pub fn ansi(&self, slot: u8) -> Color {
        let own = ANSI_COLOURS[usize::from(slot % 16)];
        self.ansi[usize::from(slot % 16)].unwrap_or(own)
    }

    /// **The theme `tui.theme` names**, from yours (`<dir>/<name>.toml`) or
    /// the shipped ones — [`load_from`](Self::load_from) with nothing
    /// imported.
    pub fn load(name: &str, dir: Option<&Path>) -> Result<(Theme, Vec<String>), String> {
        let places = Places {
            yours: dir.map(Path::to_path_buf),
            imported: Vec::new(),
        };
        Theme::load_from(name, &places)
    }

    /// **The theme named `name`, wherever `places` hold it** — yours, a
    /// shipped one, an imported palette. `Ok` holds what it said that could
    /// not be honoured, to be said at load; `Err`, why there is no theme (the
    /// caller keeps the one it has).
    pub fn load_from(name: &str, places: &Places) -> Result<(Theme, Vec<String>), String> {
        let (entries, ansi, notes) = theme_entries(name, places, 0)?;
        let theme = Theme::resolve(name, &entries, &ansi);
        let mut notes: Vec<String> = notes
            .into_iter()
            .chain(theme.taken_hues())
            .chain(theme.cursor_unsent())
            .collect();
        // A bound: each note is a notice and a log line, and a file of
        // misspellings is a megabyte of them (review of f50fda5, Reviewer B).
        if notes.len() > MOST_NOTES {
            let more = notes.len() - (MOST_NOTES - 1);
            notes.truncate(MOST_NOTES - 1);
            notes.push(format!("theme `{name}`: and {more} more like these"));
        }
        Ok((theme, notes))
    }

    /// Each role's paint from a theme's entries: its own name, else the
    /// nearest prefix named (`function` paints `function.call`); in the
    /// decision pane, a `decision.` entry first, else the code one.
    ///
    /// **Then the terminal's colours as the theme says them** (`ansi`): a
    /// role painted `magenta` — the shipped theme's, inherited — is the
    /// theme's magenta. So a theme of sixteen colours, as ghostty's are, is
    /// a whole theme, every role drawn in its palette.
    ///
    /// **A ground owns the cursor**: `ui.cursor` unnamed, a theme that paints
    /// its own ground draws the cursor in that ground's text — the block a
    /// page's reversed cell shows — since the terminal's was chosen for the
    /// terminal's ground. Not a prefix's fallback: the text's `fg` becomes
    /// the block's `bg`. Named `"default"`, it is the terminal's again.
    fn resolve(name: &str, entries: &HashMap<String, Paint>, ansi: &Ansi) -> Theme {
        let mut code = [Paint::default(); ROLES];
        let mut decision = [Paint::default(); ROLES];
        let remap = |paint: Paint| Paint {
            fg: paint.fg.map(|c| said(c, ansi)),
            bg: paint.bg.map(|c| said(c, ansi)),
            ..paint
        };
        for &ink in Ink::ALL {
            let own = nearest(entries, "", ink.name()).unwrap_or_default();
            code[ink as usize] = remap(own);
            decision[ink as usize] = remap(nearest(entries, DECISION, ink.name()).unwrap_or(own));
        }
        let ground = code[Ink::Background as usize];
        if nearest(entries, "", Ink::Cursor.name()).is_none() && ground.bg.is_some() {
            let cursor = Paint {
                bg: ground.fg,
                ..Paint::default()
            };
            code[Ink::Cursor as usize] = cursor;
            decision[Ink::Cursor as usize] = cursor;
        }
        Theme {
            name: name.to_string(),
            code,
            decision,
            ansi: *ansi,
        }
    }

    /// **A cursor the theme colours and the terminal cannot be told**, said:
    /// a terminal colour's name has no value Shannon knows, and OSC 12 takes
    /// one — so the cursor stays the terminal's, the faint one a ground
    /// was meant to replace.
    fn cursor_unsent(&self) -> Option<String> {
        let paint = self.paint(Context::Code, Ink::Cursor);
        (paint.bg.or(paint.fg).is_some() && self.cursor().is_none()).then(|| {
            format!(
                "theme `{}`: the cursor's colour is a terminal colour's name, and a terminal \
                 takes its cursor's only as #rrggbb — it keeps the terminal's own; give \
                 `ui.cursor` one",
                self.name
            )
        })
    }

    /// **A role the decision pane paints in a colour its own marks use**,
    /// said: green, yellow and red there are the gate's answer — a string
    /// painted green reads as "this runs" — and magenta a withheld value.
    ///
    /// **Every role, from the type**: the pane paints a command's shell and
    /// a Write's diff in its file's syntax, and this asked only of the
    /// shell's (a list beside the lexer) while the shipped theme painted a
    /// diff's types yellow (review of f50fda5, Reviewers A and C). Not
    /// [`MARKS`]: they carry a severity on purpose.
    fn taken_hues(&self) -> Vec<String> {
        let mut notes: Vec<String> = Ink::ALL
            .iter()
            .copied()
            .filter(|ink| !MARKS.iter().any(|mark| ink.name().starts_with(mark)))
            .filter(|&ink| {
                self.paint(Context::Decision, ink)
                    .fg
                    .is_some_and(|c| self.is_taken(c))
            })
            .map(|ink| {
                format!(
                    "theme `{}`: the decision pane paints `{}` in a colour its own marks use — \
                     there, green, yellow and red say whether a command runs, and magenta \
                     marks a withheld value; give `decision.{}` another",
                    self.name,
                    ink.name(),
                    ink.name()
                )
            })
            .collect();
        notes.extend(self.verdicts_alike());
        notes
    }

    /// **A verdict scale with two steps alike**, said: the verdict's colours
    /// are the theme's to choose (Steven, 10-14: every colour), and a theme
    /// painting `ui.danger` as `ui.safe` would draw a refusal as an allow.
    /// Said, never refused — the colours are yours.
    fn verdicts_alike(&self) -> Vec<String> {
        let code = |ink: Ink| self.paint(Context::Code, ink);
        // The scale's colours, and the decision pane's two fills' grounds.
        let pairs = [
            (
                Ink::Safe,
                Ink::Caution,
                code(Ink::Safe).fg,
                code(Ink::Caution).fg,
            ),
            (
                Ink::Safe,
                Ink::Danger,
                code(Ink::Safe).fg,
                code(Ink::Danger).fg,
            ),
            (
                Ink::Caution,
                Ink::Danger,
                code(Ink::Caution).fg,
                code(Ink::Danger).fg,
            ),
            (
                Ink::CautionFill,
                Ink::DangerFill,
                code(Ink::CautionFill).bg,
                code(Ink::DangerFill).bg,
            ),
        ];
        pairs
            .into_iter()
            .filter(|(.., a, b)| alike(*a, *b))
            .map(|(one, other, ..)| {
                format!(
                    "theme `{}`: `{}` and `{}` are one colour — a verdict's steps are told \
                     apart by colour alone",
                    self.name,
                    one.name(),
                    other.name()
                )
            })
            .collect()
    }

    /// Is `colour` one the decision pane marks with — this theme's verdict
    /// colours (`ui.safe`, `ui.caution`, `ui.danger`), a terminal colour in
    /// either shade of one; or exactly a withheld value's (`ui.secret`,
    /// `ui.operand`: its light shade is the shell's expansion, near it on
    /// purpose)?
    fn is_taken(&self, colour: Color) -> bool {
        let fg = |ink| self.paint(Context::Code, ink).fg;
        let verdict = [fg(Ink::Safe), fg(Ink::Caution), fg(Ink::Danger)];
        let withheld = [fg(Ink::Secret), fg(Ink::Operand)];
        let hue = ansi_slot(colour).map(|slot| slot % 8);
        withheld.contains(&Some(colour))
            || verdict.iter().flatten().any(|&v| {
                v == colour || hue.is_some_and(|h| ansi_slot(v).map(|s| s % 8) == Some(h))
            })
    }
}

/// The roles whose colour IS a severity — a line a write adds or removes, a
/// diagnostic — and so may wear the verdict's hues in the pane; and the
/// UI's own, which are chrome, not the pane's text.
const MARKS: &[&str] = &["diff.", "diagnostic.", "ui."];

/// **Two verdict colours an eye would not tell apart**: the same colour, the
/// same terminal hue in either shade, or two 24-bit colours closer than
/// `NEAR` — a red one step from green is no red. A colour left to the
/// terminal (`None`) is the terminal's to keep apart.
fn alike(a: Option<Color>, b: Option<Color>) -> bool {
    /// Squared RGB distance under which two colours read as one (≈ 40 of
    /// 441): a theme's near-greys are further apart than this.
    const NEAR: u32 = 40 * 40;
    match (a, b) {
        (Some(a), Some(b)) if a == b => true,
        (Some(Color::Rgb(r1, g1, b1)), Some(Color::Rgb(r2, g2, b2))) => {
            let d = |x: u8, y: u8| u32::from(x.abs_diff(y)).pow(2);
            d(r1, r2) + d(g1, g2) + d(b1, b2) < NEAR
        }
        (Some(a), Some(b)) => {
            matches!((ansi_slot(a), ansi_slot(b)), (Some(x), Some(y)) if x % 8 == y % 8)
        }
        _ => false,
    }
}

/// The sixteen terminal colours, by slot.
const ANSI_COLOURS: [Color; 16] = [
    Color::Black,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    Color::Gray,
    Color::DarkGray,
    Color::LightRed,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightBlue,
    Color::LightMagenta,
    Color::LightCyan,
    Color::White,
];

/// A terminal colour's slot (0-15) — `Indexed` below 16 is one too; `None`
/// for a colour past the sixteen.
fn ansi_slot(colour: Color) -> Option<u8> {
    match colour {
        Color::Indexed(n) if n < 16 => Some(n),
        other => ANSI_COLOURS
            .iter()
            .position(|&c| c == other)
            .and_then(|slot| u8::try_from(slot).ok()),
    }
}

/// `colour` as `ansi` says it: a terminal colour the theme names, its value;
/// anything else, itself.
fn said(colour: Color, ansi: &Ansi) -> Color {
    ansi_slot(colour)
        .and_then(|slot| ansi[usize::from(slot)])
        .unwrap_or(colour)
}

/// The entry for `name` under `prefix`, or its nearest named prefix.
fn nearest(entries: &HashMap<String, Paint>, prefix: &str, name: &str) -> Option<Paint> {
    let mut key = name;
    loop {
        if let Some(paint) = entries.get(&format!("{prefix}{key}")) {
            return Some(*paint);
        }
        key = &key[..key.rfind('.')?];
    }
}

/// A table of entries as a map.
fn entries(table: &[(&str, Paint)]) -> HashMap<String, Paint> {
    table
        .iter()
        .map(|(key, paint)| (key.to_string(), *paint))
        .collect()
}

/// How deep one theme may inherit another — past it, a cycle.
const MOST_INHERITED: usize = 8;

/// How many notes a theme's load says, the last of them how many more.
const MOST_NOTES: usize = 8;

/// What a theme says, read: its entries, the terminal colours it names, and
/// what it said that could not be honoured.
type Said = (HashMap<String, Paint>, Ansi, Vec<String>);

/// Declares the shipped theme files once — each a file in
/// `crates/shannon-tui/themes/`, read into the binary — so [`SHIPPED`] and
/// the directory cannot disagree (`every_theme_file_is_shipped`).
macro_rules! shipped {
    ($($name:literal,)*) => {
        /// **The themes Shannon ships**, beside `terminal`: a name and its
        /// file's text. Where their colours come from, `themes/NOTICE.md`.
        const SHIPPED: &[(&str, &str)] = &[
            $(($name, include_str!(concat!("../themes/", $name, ".toml"))),)*
        ];
    };
}

shipped! {
    "catppuccin-mocha",
    "catppuccin-macchiato",
    "catppuccin-frappe",
    "catppuccin-latte",
    "tokyonight-night",
    "tokyonight-storm",
    "tokyonight-moon",
    "tokyonight-day",
    "gruvbox-dark",
    "gruvbox-light",
    "nord",
    "dracula",
    "one-dark",
    "solarized-dark",
    "solarized-light",
    "rose-pine",
    "rose-pine-moon",
    "rose-pine-dawn",
    "kanagawa-wave",
    "kanagawa-dragon",
    "kanagawa-lotus",
}

/// A shipped theme file's text, by name.
fn shipped(name: &str) -> Option<&'static str> {
    SHIPPED
        .iter()
        .find(|(shipped, _)| *shipped == name)
        .map(|(_, text)| *text)
}

/// **Every theme Shannon ships**, `terminal` first.
pub fn shipped_names() -> impl Iterator<Item = &'static str> {
    std::iter::once(Theme::DEFAULT).chain(SHIPPED.iter().map(|(name, _)| *name))
}

/// **Where themes are found**: your `themes/` directory, beside your config,
/// and the directories `tui.theme_import` names — terminal palettes, ghostty's
/// by default (`ghostty::dirs`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Places {
    pub yours: Option<PathBuf>,
    pub imported: Vec<PathBuf>,
}

/// Where a theme was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// One Shannon ships (`terminal` among them).
    Shipped,
    /// A file in your `themes/`.
    Yours,
    /// A terminal palette in an imported directory.
    Imported(PathBuf),
}

impl Places {
    /// **Every theme there is**, each name once, from where a lookup finds
    /// it — `terminal` and the shipped ones, yours (one of yours named for a
    /// shipped theme is yours), then the imported ones, each directory's in
    /// name order.
    pub fn themes(&self) -> Vec<(String, Source)> {
        let mut out: Vec<(String, Source)> = shipped_names()
            .map(|name| (name.to_string(), Source::Shipped))
            .collect();
        if let Some(dir) = &self.yours {
            for file in ghostty::listed(dir) {
                let Some(name) = file.strip_suffix(".toml").filter(|n| is_plain(n)) else {
                    continue;
                };
                match out.iter_mut().find(|(n, _)| n == name) {
                    Some(found) => found.1 = Source::Yours,
                    None => out.push((name.to_string(), Source::Yours)),
                }
            }
        }
        for dir in &self.imported {
            for name in ghostty::listed(dir) {
                if !out.iter().any(|(n, _)| *n == name) {
                    out.push((name, Source::Imported(dir.clone())));
                }
            }
        }
        out
    }
}

/// Is `name` a plain one — letters, digits, `-` and `_` — what a file of
/// yours or a shipped theme is named by? Never a path.
fn is_plain(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A theme's entries by name — shipped, a file of yours and what it
/// inherits, or an imported palette — the terminal colours it says, and what
/// it said that could not be honoured. A file's own entries and colours over
/// what it inherits.
fn theme_entries(name: &str, places: &Places, depth: usize) -> Result<Said, String> {
    if name == Theme::DEFAULT {
        return Ok((entries(TERMINAL), TERMINAL_ANSI, Vec::new()));
    }
    if depth >= MOST_INHERITED {
        return Err(format!(
            "theme `{name}`: inherits too deep — a theme inheriting itself?"
        ));
    }
    // A plain name: yours first, then a shipped one — a file of yours named
    // for a shipped theme is yours, as a program model of yours replaces the
    // shipped one. A name, never a path: it names a file in one directory.
    let path = places
        .yours
        .as_ref()
        .filter(|_| is_plain(name))
        .map(|dir| dir.join(format!("{name}.toml")));
    // The config's own reader: a theme named at a FIFO or `/dev/zero` would
    // otherwise wait forever on the thread that draws the screen.
    let yours = match &path {
        Some(path) => shannon_core::config::read_file(path)
            .map_err(|why| format!("theme `{name}`: {} — {why}", path.display()))?,
        None => None,
    };
    let text = match (yours, shipped(name).filter(|_| is_plain(name))) {
        (Some(text), _) => text,
        (None, Some(text)) => text.to_string(),
        (None, None) => return imported(name, places),
    };
    let file = parse(name, &text)?;
    let (mut entries, mut ansi, mut notes) = theme_entries(&file.inherits, places, depth + 1)?;
    entries.extend(file.entries);
    for (slot, colour) in ansi.iter_mut().zip(file.ansi) {
        *slot = colour.or(*slot);
    }
    notes.extend(file.notes);
    Ok((entries, ansi, notes))
}

/// **An imported terminal palette named exactly `name`** — found in a
/// directory's listing, never joined onto it from what was asked, so a name
/// with a `/` or a `..` matches no file and reaches nothing — made a theme
/// over `terminal` (`ghostty::entries`).
fn imported(name: &str, places: &Places) -> Result<Said, String> {
    for dir in &places.imported {
        if !ghostty::listed(dir).iter().any(|file| file == name) {
            continue;
        }
        let path = dir.join(name);
        let text = match shannon_core::config::read_file(&path) {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(why) => return Err(format!("theme `{name}`: {} — {why}", path.display())),
        };
        let (palette, notes) = ghostty::read(name, &text);
        let mut entries = entries(TERMINAL);
        entries.extend(ghostty::entries(&palette));
        return Ok((entries, palette.ansi, notes));
    }
    let looked = match (&places.yours, is_plain(name)) {
        (Some(dir), true) => format!("no {}", dir.join(format!("{name}.toml")).display()),
        (None, true) => "no config directory".to_string(),
        (_, false) => "a theme of yours or a shipped one is named by letters, digits, `-` \
                       and `_`"
            .to_string(),
    };
    Err(format!(
        "theme `{name}`: {looked}, and none is shipped or imported by that name (`:help \
         shannon-themes` lists them)"
    ))
}

/// A theme file, read: what it inherits, its entries, the terminal colours
/// its `[palette]` names, and what it said that names nothing.
struct File {
    inherits: String,
    entries: HashMap<String, Paint>,
    ansi: Ansi,
    notes: Vec<String>,
}

/// **A theme file** — TOML, a role per key, helix's shape:
///
/// ```toml
/// inherits = "terminal"          # the default
/// keyword = "magenta"            # a terminal colour, by name
/// comment = { fg = 8, italic = true }
/// "function.call" = "#61afef"    # 24-bit
/// "decision.string" = "light-cyan"
///
/// "ui.background" = { fg = "#cdd6f4", bg = "#1e1e2e" }
///
/// [palette]                      # names for colours, used above
/// lavender = "#b3a1e6"
/// magenta = "#cba6f7"            # and what the terminal's magenta is
/// ```
///
/// **A `[palette]` entry named for a terminal colour** (`black` …
/// `bright-white`) says what that colour is for every role — the
/// inherited ones too — so sixteen entries and a background make a whole
/// theme of one drawn for the terminal's palette (`Theme::resolve`).
///
/// An entry that names no role, or a colour that is not one, is said, not
/// skipped in silence; a file that is not TOML is refused whole — its error
/// by line, never quoting it (`Config::describe_parse_error`: every notice
/// is logged).
///
/// **A role is one key however TOML spells it**: `"function.call" = …`,
/// unquoted `function.call = …` (TOML's table `function = { call = … }`),
/// and `[decision] string = …` are each the dotted key ([`flatten`]). The
/// unquoted one was read as `function`'s colour — empty — and wiped it
/// (review of f50fda5, Reviewers A and C).
fn parse(name: &str, text: &str) -> Result<File, String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        format!(
            "theme `{name}`: not TOML — {}",
            shannon_core::Config::describe_parse_error(&e, text)
        )
    })?;
    let mut notes = Vec::new();
    let mut palette = HashMap::new();
    let mut ansi = TERMINAL_ANSI;
    match table.get("palette") {
        Some(toml::Value::Table(names)) => {
            for (key, value) in names {
                match colour(value, &HashMap::new()) {
                    Ok(Some(colour)) => {
                        // A terminal colour's name says what that colour is
                        // — for every role, the inherited ones too.
                        let named = colour_word(key).ok().flatten().and_then(ansi_slot);
                        if let Some(slot) = named {
                            ansi[usize::from(slot)] = Some(colour);
                        }
                        palette.insert(key.clone(), colour);
                    }
                    Ok(None) => notes.push(format!(
                        "theme `{name}`: palette `{key}` — `default` is not a colour to name"
                    )),
                    Err(why) => notes.push(format!("theme `{name}`: palette `{key}` — {why}")),
                }
            }
        }
        Some(_) => notes.push(format!("theme `{name}`: `palette` is a table of names")),
        None => {}
    }
    let inherits = match table.get("inherits") {
        Some(toml::Value::String(parent)) => parent.clone(),
        Some(_) => {
            notes.push(format!(
                "theme `{name}`: `inherits` names a theme, in quotes — inheriting `{}`",
                Theme::DEFAULT
            ));
            Theme::DEFAULT.to_string()
        }
        None => Theme::DEFAULT.to_string(),
    };
    let mut entries = HashMap::new();
    let mut roles = Vec::new();
    for (key, value) in &table {
        if key != "inherits" && key != "palette" {
            flatten(key.clone(), value, &mut roles);
        }
    }
    for (key, value) in roles {
        let role = key.strip_prefix(DECISION).unwrap_or(&key);
        if !names_a_role(role) {
            notes.push(format!(
                "theme `{name}`: `{key}` names no role (`:help themes` lists them)"
            ));
            continue;
        }
        match paint(value, &palette) {
            Ok(paint) => {
                entries.insert(key, paint);
            }
            Err(why) => notes.push(format!("theme `{name}`: `{key}` — {why}")),
        }
    }
    Ok(File {
        inherits,
        entries,
        ansi,
        notes,
    })
}

/// The fields a colour's table may have ([`paint`]); a table with none of
/// them holds roles under its key instead ([`flatten`]).
const PAINT_FIELDS: &[&str] = &["fg", "bg", "bold", "italic", "dim", "modifiers"];

/// `value` under the dotted `key`, as the role entries it holds: itself, or
/// — a table with no colour's field — each entry under it, `key.` before
/// its name.
fn flatten<'a>(key: String, value: &'a toml::Value, out: &mut Vec<(String, &'a toml::Value)>) {
    match value {
        toml::Value::Table(inner)
            if !inner
                .keys()
                .any(|field| PAINT_FIELDS.contains(&field.as_str())) =>
        {
            for (field, value) in inner {
                flatten(format!("{key}.{field}"), value, out);
            }
        }
        _ => out.push((key, value)),
    }
}

/// Is `key` a role's name, or a prefix of one (`function`, `markup.raw`)?
fn names_a_role(key: &str) -> bool {
    Ink::ALL.iter().any(|ink| {
        let name = ink.name();
        name == key || name.starts_with(&format!("{key}."))
    })
}

/// A paint, from a colour alone or a table of one and its emphasis — `bold
/// = true`, or helix's `modifiers = ["bold", "italic"]`. A field it does not
/// know is said: `bolt = true` was dropped in silence (Reviewer C).
fn paint(value: &toml::Value, palette: &HashMap<String, Color>) -> Result<Paint, String> {
    let toml::Value::Table(fields) = value else {
        return Ok(Paint {
            fg: colour(value, palette)?,
            ..Paint::default()
        });
    };
    let mut paint = Paint::default();
    for (field, value) in fields {
        let flag = || {
            value
                .as_bool()
                .ok_or_else(|| format!("`{field}` is `true` or `false`"))
        };
        match field.as_str() {
            "fg" => paint.fg = colour(value, palette)?,
            "bg" => paint.bg = colour(value, palette)?,
            "bold" => paint.bold = flag()?,
            "italic" => paint.italic = flag()?,
            "dim" => paint.dim = flag()?,
            "modifiers" => {
                let names = value.as_array().ok_or("`modifiers` is a list")?;
                for modifier in names {
                    match modifier.as_str() {
                        Some("bold") => paint.bold = true,
                        Some("italic") => paint.italic = true,
                        Some("dim") => paint.dim = true,
                        _ => {
                            return Err(format!(
                                "`{modifier}`: the modifiers are bold, italic and dim"
                            ));
                        }
                    }
                }
            }
            _ => {
                return Err(format!(
                    "`{field}` is not a colour's field ({})",
                    PAINT_FIELDS.join(", ")
                ));
            }
        }
    }
    Ok(paint)
}

/// **A colour as a theme writes one**: a terminal colour's name (`magenta`,
/// `bright-black`), its number (0-255), `#rrggbb`, a `[palette]` name, or
/// `default` — the terminal's own text colour (`None`).
fn colour(value: &toml::Value, palette: &HashMap<String, Color>) -> Result<Option<Color>, String> {
    if let Some(n) = value.as_integer() {
        return u8::try_from(n)
            .map(|n| Some(Color::Indexed(n)))
            .map_err(|_| format!("{n} is not a terminal colour (0-255)"));
    }
    let Some(text) = value.as_str() else {
        return Err("a colour is a name, a number or `#rrggbb`".into());
    };
    if let Some(colour) = palette.get(text) {
        return Ok(Some(*colour));
    }
    if let Some(hex) = text.strip_prefix('#') {
        let byte = |at: usize| {
            hex.get(at..at + 2)
                .and_then(|b| u8::from_str_radix(b, 16).ok())
        };
        return match (hex.len(), byte(0), byte(2), byte(4)) {
            (6, Some(r), Some(g), Some(b)) => Ok(Some(Color::Rgb(r, g, b))),
            _ => Err(format!("`{text}` is not `#rrggbb`")),
        };
    }
    Ok(Some(match text {
        "default" | "none" => return Ok(None),
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" | "gray" | "grey" => Color::Gray,
        "bright-black" | "dark-gray" | "dark-grey" => Color::DarkGray,
        "bright-red" | "light-red" => Color::LightRed,
        "bright-green" | "light-green" => Color::LightGreen,
        "bright-yellow" | "light-yellow" => Color::LightYellow,
        "bright-blue" | "light-blue" => Color::LightBlue,
        "bright-magenta" | "light-magenta" => Color::LightMagenta,
        "bright-cyan" | "light-cyan" => Color::LightCyan,
        "bright-white" => Color::White,
        _ => return Err(format!("`{text}` is not a colour this knows")),
    }))
}

/// **A stop of the status line's `ctx`**: from this percent of the context
/// window up, `ctx` takes this colour (`None`: the row's own).
pub(crate) type CtxStop = (u8, Option<Color>);

/// **Read `tui.ctx_colors`** — each `"percent:colour"` a stop, the colour as
/// a theme writes one ([`colour`]), sorted by percent so the draw takes the
/// last stop at or below the gauge (`view::context_style`). What an entry
/// cannot say is noted and the entry left out, never guessed.
pub(crate) fn ctx_stops(entries: &[String]) -> (Vec<CtxStop>, Vec<String>) {
    let mut stops = Vec::new();
    let mut notes = Vec::new();
    for entry in entries {
        match ctx_stop(entry) {
            Ok(stop) => stops.push(stop),
            Err(why) => notes.push(format!("tui.ctx_colors: `{entry}` — {why}")),
        }
    }
    // Stable, so of two stops at one percent the later-written wins.
    stops.sort_by_key(|(at, _)| *at);
    (stops, notes)
}

fn ctx_stop(entry: &str) -> Result<CtxStop, String> {
    let Some((at, hue)) = entry.split_once(':') else {
        return Err("a stop is `percent:colour`, as `70:yellow`".into());
    };
    let at = at.trim().trim_end_matches('%');
    let at = at
        .parse::<u8>()
        .ok()
        .filter(|p| *p <= 100)
        .ok_or_else(|| format!("`{at}` is not a percent (0-100)"))?;
    Ok((at, colour_word(hue.trim())?))
}

/// **A colour written as one word** — `yellow`, `208`, `#ff8800`, `default` —
/// read as a theme reads one ([`colour`]): `tui.ctx_colors`' stops and a
/// status format's `(style)` say colours in the theme's language, not a
/// second one.
pub(crate) fn colour_word(word: &str) -> Result<Option<Color>, String> {
    let value = match word.parse::<i64>() {
        Ok(n) => toml::Value::Integer(n),
        Err(_) => toml::Value::String(word.to_string()),
    };
    colour(&value, &HashMap::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The shipped table names only roles** — the check a user's file gets
    /// (`parse`: `decision.` stripped, then `names_a_role`), which `TERMINAL`
    /// went past to `entries()`. `ui.bar.waiting` misspelt took the waiting
    /// bar's colour away with the crate green (review stop 12, T7 12).
    #[test]
    fn every_shipped_terminal_entry_names_a_role() {
        for (key, _) in TERMINAL {
            let role = key.strip_prefix(DECISION).unwrap_or(key);
            assert!(names_a_role(role), "`{key}` names no role");
        }
    }

    /// **A `ctx` stop is read as a theme reads a colour** — a name, a number
    /// or `#rrggbb`, from its percent up, in percent order whatever order it
    /// was written in — and what cannot be read is said, not guessed.
    #[test]
    fn ctx_stops_are_read_in_percent_order_and_bad_ones_said() {
        let entries = [
            "90:#ff0000",
            "70%:yellow",
            "80: 208",
            "50:default",
            "fifty:red",
            "120:red",
            "60:mauve",
            "red",
        ]
        .map(String::from);
        let (stops, notes) = ctx_stops(&entries);
        assert_eq!(
            stops,
            [
                (50, None),
                (70, Some(Color::Yellow)),
                (80, Some(Color::Indexed(208))),
                (90, Some(Color::Rgb(255, 0, 0))),
            ]
        );
        assert_eq!(notes.len(), 4, "{notes:?}");
        for bad in ["fifty:red", "120:red", "60:mauve", "`red`"] {
            assert!(notes.iter().any(|n| n.contains(bad)), "{bad}: {notes:?}");
        }
    }

    /// **The shipped theme names terminal colours only** — never a value —
    /// so the terminal's own palette decides what each looks like (Steven,
    /// 10-06: "inherit colors in a similar way").
    #[test]
    fn the_shipped_theme_inherits_the_terminals_colours() {
        // `palette.rs`'s two truecolor holdouts, named there as the
        // exception — chosen against a dark ground and not yet guessed at.
        let holdouts = ["ui.graph.shared", "ui.graph.mark"];
        for (key, paint) in TERMINAL {
            if holdouts.contains(key) {
                continue;
            }
            for colour in [paint.fg, paint.bg] {
                assert!(
                    !matches!(colour, Some(Color::Rgb(..)) | Some(Color::Indexed(16..))),
                    "{key}: {paint:?}"
                );
            }
        }
        let theme = Theme::terminal();
        assert_eq!(
            theme.paint(Context::Code, Ink::Keyword).fg,
            Some(Color::Magenta)
        );
        assert_eq!(
            theme.paint(Context::Code, Ink::String).fg,
            Some(Color::Green)
        );
    }

    /// **A role falls back to its nearest named prefix**, and the decision
    /// pane to the code entry.
    #[test]
    fn a_role_falls_back_to_its_prefix_and_the_pane_to_the_code() {
        let theme = Theme::terminal();
        // `constant.builtin` is not named: `constant` paints it.
        assert_eq!(
            theme.paint(Context::Code, Ink::ConstantBuiltin),
            theme.paint(Context::Code, Ink::Constant)
        );
        // The pane names its own; a role it does not name is the code one.
        assert_eq!(
            theme.paint(Context::Decision, Ink::FunctionCall).fg,
            Some(Color::White)
        );
        assert_eq!(
            theme.paint(Context::Decision, Ink::Comment),
            theme.paint(Context::Code, Ink::Comment)
        );
        // A role no entry reaches is the terminal's text, unstyled.
        assert_eq!(theme.paint(Context::Code, Ink::Variable), Paint::default());
    }

    /// **The rule around your message is `ui.border` until a theme names
    /// it** — left out of the shipped table on purpose, so a light theme that
    /// repaints `ui.border` (the shipped grey is its light "white") frames
    /// your message in the colour it chose, without naming it twice.
    #[test]
    fn the_rule_around_your_message_is_the_border_unless_named() {
        let terminal = Theme::terminal();
        assert_eq!(terminal.ui(Ink::BorderUser), terminal.ui(Ink::Border));
        let (latte, _) = Theme::load("catppuccin-latte", None).unwrap();
        assert_ne!(
            latte.ui(Ink::Border),
            terminal.ui(Ink::Border),
            "premise: latte repaints the border"
        );
        assert_eq!(latte.ui(Ink::BorderUser), latte.ui(Ink::Border));
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("framed.toml"),
            "inherits = \"terminal\"\n\"ui.border\" = \"red\"\n\"ui.border.user\" = \"green\"\n",
        )
        .unwrap();
        let (framed, _) = Theme::load("framed", Some(dir.path())).unwrap();
        assert_eq!(framed.fg(Ink::BorderUser), Color::Green);
        assert_eq!(framed.fg(Ink::Border), Color::Red);
    }

    /// **The shipped decision pane keeps the shell off the verdict's hues
    /// and the withheld value's magenta** — the claim that was `palette`'s
    /// (review of 716f4f9, C-L6), now checked of any theme at load.
    #[test]
    fn the_shipped_pane_keeps_off_the_verdicts_hues() {
        assert!(Theme::terminal().taken_hues().is_empty());
        let dir = tempfile::TempDir::new().unwrap();
        let said = |entry: &str| {
            std::fs::write(dir.path().join("loud.toml"), format!("{entry}\n")).unwrap();
            Theme::load("loud", Some(dir.path())).unwrap().1
        };
        for (taken, role) in [
            ("\"decision.string\" = \"green\"", "string"),
            ("\"decision.string\" = \"bright-red\"", "string"),
            ("\"decision.string\" = 3", "string"),
            ("\"decision.string\" = \"magenta\"", "string"),
            ("\"decision.keyword\" = \"red\"", "keyword"),
            // Every role the pane paints, not the shell's alone: a Write's
            // diff shows comments, and the pane falls back to the code map.
            ("comment = \"green\"", "comment"),
            ("\"decision.type\" = \"yellow\"", "type"),
        ] {
            let notes = said(taken);
            assert!(
                notes
                    .iter()
                    .any(|n| n.contains(&format!("`{role}`")) && n.contains("its own marks")),
                "{taken}: {notes:?}"
            );
        }
        // The expansion's light magenta is near the secret's on purpose; a
        // 24-bit colour is not judged; a code entry does not reach a role
        // the shipped theme names for the pane — the more specific entry
        // wins, inherited or not; and a mark's colour is its severity.
        for free in [
            "\"decision.string\" = \"light-magenta\"",
            "\"decision.string\" = \"#00ff00\"",
            "string = \"green\"",
            "\"diagnostic.warning\" = \"yellow\"",
            "\"diff.plus\" = \"green\"",
        ] {
            assert!(said(free).is_empty(), "{free}");
        }
    }

    /// **A role is one key however TOML spells it** — unquoted dotted, a
    /// `[decision]` table — and what a file writes that is not a colour's
    /// field, a palette's colour or a theme's name is said (review of
    /// f50fda5, Reviewers A and C: each was dropped in silence, and the
    /// unquoted key wiped `function`'s colour).
    #[test]
    fn a_role_is_one_key_however_toml_spells_it() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("spelled.toml"),
            r##"
inherits = 3
function.call = "red"
keyword = { fg = "blue", modifiers = ["bold", "italic"] }
comment = { fg = "cyan", bolt = true }
type = { modifiers = ["blink"] }

[decision]
string = "light-blue"

[palette]
lav = "#zzzzzz"
"##,
        )
        .unwrap();
        let (theme, notes) = Theme::load("spelled", Some(dir.path())).unwrap();
        let code = |ink| theme.paint(Context::Code, ink);
        assert_eq!(code(Ink::FunctionCall).fg, Some(Color::Red));
        assert_eq!(code(Ink::Function).fg, Some(Color::Blue), "`function` kept");
        assert!(code(Ink::Keyword).bold && code(Ink::Keyword).italic);
        assert_eq!(
            theme.paint(Context::Decision, Ink::String).fg,
            Some(Color::LightBlue)
        );
        for said in ["`bolt`", "blink", "palette `lav`", "`inherits`"] {
            assert!(notes.iter().any(|n| n.contains(said)), "{said}: {notes:?}");
        }
        // Not taken as a colour: the comment keeps the shipped one.
        assert_eq!(
            code(Ink::Comment),
            Theme::terminal().paint(Context::Code, Ink::Comment)
        );
    }

    /// **A name, never a path** — held by a file that IS there, so the
    /// refusal is the guard's and not a missing file's (Reviewer C: the
    /// guard deleted passed, `../etc/passwd.toml` being absent anyway).
    #[test]
    fn a_theme_is_named_never_pathed() {
        let base = tempfile::TempDir::new().unwrap();
        let themes = base.path().join("themes");
        std::fs::create_dir(&themes).unwrap();
        std::fs::write(base.path().join("outside.toml"), "keyword = \"red\"\n").unwrap();
        let why = Theme::load("../outside", Some(&themes)).unwrap_err();
        assert!(why.contains("letters, digits"), "{why}");
    }

    /// **What a load says is bounded** — a file of misspellings says a few,
    /// and how many more.
    #[test]
    fn a_loads_notes_are_bounded() {
        let dir = tempfile::TempDir::new().unwrap();
        let text: String = (0..40).map(|i| format!("nope{i} = \"red\"\n")).collect();
        std::fs::write(dir.path().join("many.toml"), text).unwrap();
        let (_, notes) = Theme::load("many", Some(dir.path())).unwrap();
        assert_eq!(notes.len(), MOST_NOTES);
        assert!(notes.last().unwrap().contains("33 more"), "{notes:?}");
    }

    /// **A theme file**: inherits the shipped one, names colours every way,
    /// and says what names nothing.
    #[test]
    fn a_theme_file_is_read_and_says_what_it_cannot_honour() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mine.toml"),
            r##"
keyword = "bright-magenta"
comment = { fg = 244, italic = true }
"function.call" = "#61afef"
string = "lavender"
"decision.operator" = "default"
keywrd = "red"
type = "ultraviolet"

[palette]
lavender = "#b3a1e6"
"##,
        )
        .unwrap();
        let (theme, notes) = Theme::load("mine", Some(dir.path())).unwrap();
        let code = |ink| theme.paint(Context::Code, ink);
        assert_eq!(code(Ink::Keyword).fg, Some(Color::LightMagenta));
        assert_eq!(code(Ink::Comment).fg, Some(Color::Indexed(244)));
        assert!(code(Ink::Comment).italic);
        assert_eq!(
            code(Ink::FunctionCall).fg,
            Some(Color::Rgb(0x61, 0xaf, 0xef))
        );
        assert_eq!(code(Ink::String).fg, Some(Color::Rgb(0xb3, 0xa1, 0xe6)));
        assert_eq!(theme.paint(Context::Decision, Ink::Operator).fg, None);
        // Inherited: what it does not name is the shipped theme's.
        assert_eq!(
            code(Ink::Number),
            Theme::terminal().paint(Context::Code, Ink::Number)
        );
        assert!(
            notes.iter().any(|n| n.contains("`keywrd` names no role")),
            "{notes:?}"
        );
        assert!(notes.iter().any(|n| n.contains("ultraviolet")), "{notes:?}");
    }

    /// **No theme it cannot vouch for**: a missing file, a path for a name,
    /// a file that is not TOML, a cycle — each an error, never a panic.
    #[test]
    fn a_theme_that_cannot_be_read_is_an_error() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(Theme::load("absent", Some(dir.path())).is_err());
        assert!(Theme::load("../etc/passwd", Some(dir.path())).is_err());
        std::fs::write(dir.path().join("bad.toml"), "keyword = [").unwrap();
        assert!(Theme::load("bad", Some(dir.path())).is_err());
        std::fs::write(dir.path().join("loop.toml"), "inherits = \"loop\"\n").unwrap();
        assert!(Theme::load("loop", Some(dir.path())).is_err());
        assert!(
            Theme::load("terminal", None).is_ok(),
            "the shipped one needs no file"
        );
    }

    /// **A theme is read as a config file is** (`config::read_file`): a
    /// device is refused, never read — `/dev/zero` would grow without end,
    /// and a FIFO would wait for a writer, on the thread that draws.
    /// `/dev/null` reads as an empty theme to a plain read, which is how
    /// this tells the two readers apart without hanging either.
    #[cfg(unix)]
    #[test]
    fn a_theme_that_is_not_a_regular_file_is_refused() {
        let dir = tempfile::TempDir::new().unwrap();
        std::os::unix::fs::symlink("/dev/null", dir.path().join("null.toml")).unwrap();
        let why = Theme::load("null", Some(dir.path())).unwrap_err();
        assert!(why.contains("not a regular file"), "{why}");
    }

    /// A ghostty theme file of `ansi` (each `#rrggbb`) on `base`.
    fn ghostty_file(ansi: [&str; 16], base: &str, text: &str) -> String {
        let mut out: String = ansi
            .iter()
            .enumerate()
            .map(|(n, hex)| format!("palette = {n}={hex}\n"))
            .collect();
        out.push_str(&format!("background = {base}\nforeground = {text}\n"));
        out
    }

    const MOCHA: [&str; 16] = [
        "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8",
        "#585b70", "#f37799", "#89d88b", "#ebd391", "#74a8fc", "#f2aede", "#6bd7ca", "#bac2de",
    ];

    /// **An imported terminal palette is a theme by its file's name** —
    /// spaces and all — every role in its colours, its ground, and nothing to
    /// say about a palette drawn for a terminal.
    #[test]
    fn an_imported_palette_is_a_theme_by_its_files_name() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("Catppuccin Mocha"),
            ghostty_file(MOCHA, "#1e1e2e", "#cdd6f4"),
        )
        .unwrap();
        let places = Places {
            yours: None,
            imported: vec![dir.path().to_path_buf()],
        };
        let (theme, notes) = Theme::load_from("Catppuccin Mocha", &places).expect("loads");
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(theme.fg(Ink::Keyword), Color::Rgb(0xf5, 0xc2, 0xe7));
        assert_eq!(theme.fg(Ink::BorderFocused), Color::Rgb(0x94, 0xe2, 0xd5));
        assert_eq!(
            theme.ui(Ink::Background).bg,
            Some(Color::Rgb(0x1e, 0x1e, 0x2e))
        );
        assert_eq!(
            theme.ui(Ink::DangerFill).fg,
            Some(Color::Rgb(0x1e, 0x1e, 0x2e)),
            "dark text on the fill"
        );
    }

    /// **An imported name reaches no further than its directory's listing**:
    /// a path, a `..`, a file one directory over — each names nothing.
    #[test]
    fn an_imported_name_reaches_no_further_than_its_listing() {
        let base = tempfile::TempDir::new().unwrap();
        let themes = base.path().join("themes");
        std::fs::create_dir(&themes).unwrap();
        std::fs::create_dir(themes.join("sub")).unwrap();
        let file = ghostty_file(MOCHA, "#1e1e2e", "#cdd6f4");
        std::fs::write(base.path().join("outside"), &file).unwrap();
        std::fs::write(themes.join("sub").join("inner"), &file).unwrap();
        let places = Places {
            yours: None,
            imported: vec![themes.clone()],
        };
        for name in ["../outside", "sub/inner", "sub", ".", ".."] {
            assert!(Theme::load_from(name, &places).is_err(), "{name}");
        }
        assert!(
            places.themes().iter().all(|(n, _)| n != "sub"),
            "a directory is no theme"
        );
    }

    /// **Yours, then shipped, then imported** — and the listing names each
    /// once, from where a lookup finds it.
    #[test]
    fn a_theme_is_found_yours_then_shipped_then_imported() {
        let yours = tempfile::TempDir::new().unwrap();
        let imported = tempfile::TempDir::new().unwrap();
        let file = ghostty_file(MOCHA, "#000001", "#cdd6f4");
        std::fs::write(imported.path().join("nord"), &file).unwrap();
        std::fs::write(imported.path().join("dracula"), &file).unwrap();
        std::fs::write(imported.path().join("Mine Only"), &file).unwrap();
        std::fs::write(yours.path().join("dracula.toml"), "keyword = \"#010203\"\n").unwrap();
        let places = Places {
            yours: Some(yours.path().to_path_buf()),
            imported: vec![imported.path().to_path_buf()],
        };
        let ground = |name| {
            Theme::load_from(name, &places)
                .unwrap()
                .0
                .ui(Ink::Background)
                .bg
        };
        assert_eq!(
            ground("nord"),
            Some(Color::Rgb(0x2e, 0x34, 0x40)),
            "shipped"
        );
        assert_eq!(ground("Mine Only"), Some(Color::Rgb(0, 0, 1)), "imported");
        let (dracula, _) = Theme::load_from("dracula", &places).unwrap();
        assert_eq!(dracula.fg(Ink::Keyword), Color::Rgb(1, 2, 3), "yours");
        let listed = places.themes();
        let source = |name: &str| {
            let found: Vec<&Source> = listed
                .iter()
                .filter(|(n, _)| n == name)
                .map(|(_, s)| s)
                .collect();
            assert_eq!(found.len(), 1, "{name} once: {listed:?}");
            found[0].clone()
        };
        assert_eq!(source("terminal"), Source::Shipped);
        assert_eq!(source("nord"), Source::Shipped);
        assert_eq!(source("dracula"), Source::Yours);
        assert_eq!(
            source("Mine Only"),
            Source::Imported(imported.path().to_path_buf())
        );
        assert_eq!(listed[0].0, "terminal", "first");
    }

    /// **`tui.theme_import`'s entries**: `ghostty` its directories, a path
    /// itself, `~/` the home — in the environment given, never this
    /// machine's.
    #[test]
    fn theme_import_names_directories() {
        let env = |k: &str| match k {
            "HOME" => Some("/h".to_string()),
            "XDG_CONFIG_HOME" => Some("/x".to_string()),
            "GHOSTTY_RESOURCES_DIR" => Some("/r".to_string()),
            _ => None,
        };
        let dirs = import_dirs(
            &["ghostty".into(), "~/palettes".into(), "/abs".into()],
            &env,
        );
        let expected = ["/x/ghostty/themes", "/r/themes", "/h/palettes", "/abs"];
        assert_eq!(dirs, expected.map(PathBuf::from));
    }

    /// **Every shipped theme loads, and has nothing to say** — no misspelt
    /// role, no shell on the verdict's colours, no two verdict steps alike —
    /// and grounds the frame in its own background, and so colours the
    /// cursor; the shipped `terminal` leaves both to the terminal.
    #[test]
    fn every_shipped_theme_loads_and_says_nothing() {
        let names: Vec<&str> = shipped_names().collect();
        assert!(names.len() > 20, "premise: the set is shipped: {names:?}");
        for name in names {
            let (theme, notes) = Theme::load(name, None).unwrap_or_else(|e| panic!("{e}"));
            assert!(notes.is_empty(), "{name}: {notes:?}");
            assert_eq!(theme.name(), name);
            if name != Theme::DEFAULT {
                assert!(theme.ui(Ink::Background).bg.is_some(), "{name}: a ground");
                assert!(theme.cursor().is_some(), "{name}: a cursor");
            } else {
                assert_eq!(theme.cursor(), None, "{name}: the terminal's cursor");
            }
        }
    }

    /// **A ground owns the cursor**: unnamed, it is the ground's text — the
    /// block a page's reversed cell draws — whatever the theme inherits;
    /// named, it is yours, `"default"` the terminal's own; a name a terminal
    /// cannot be told is said, never silently the faint one.
    #[test]
    fn a_ground_colours_the_cursor_unless_the_theme_says_otherwise() {
        let latte = Theme::load("catppuccin-latte", None).unwrap().0;
        let text = match latte.ui(Ink::Background).fg {
            Some(Color::Rgb(r, g, b)) => (r, g, b),
            other => panic!("premise: latte's text is 24-bit: {other:?}"),
        };
        assert_eq!(latte.cursor(), Some(text), "the ground's text");

        let dir = tempfile::TempDir::new().unwrap();
        let theme = |name: &str, body: &str| {
            std::fs::write(dir.path().join(format!("{name}.toml")), body).unwrap();
            Theme::load(name, Some(dir.path())).unwrap()
        };
        let (own, notes) = theme(
            "own",
            "inherits = \"catppuccin-latte\"\n\"ui.cursor\" = \"#dc8a78\"\n",
        );
        assert_eq!(own.cursor(), Some((0xdc, 0x8a, 0x78)), "a bare colour");
        assert!(notes.is_empty(), "{notes:?}");
        let (table, _) = theme(
            "table",
            "inherits = \"catppuccin-latte\"\n\"ui.cursor\" = { fg = \"#eff1f5\", bg = \"#dc8a78\" }\n",
        );
        assert_eq!(table.cursor(), Some((0xdc, 0x8a, 0x78)), "vim's: the bg");
        let (terminal, notes) = theme(
            "kept",
            "inherits = \"catppuccin-latte\"\n\"ui.cursor\" = \"default\"\n",
        );
        assert_eq!(terminal.cursor(), None, "asked for the terminal's");
        assert!(notes.is_empty(), "{notes:?}");
        let (inherited, _) = theme(
            "dark",
            "inherits = \"catppuccin-latte\"\n\"ui.background\" = { fg = \"#cdd6f4\", bg = \"#1e1e2e\" }\n",
        );
        assert_eq!(
            inherited.cursor(),
            Some((0xcd, 0xd6, 0xf4)),
            "derived from its own ground, not the inherited one's"
        );
        let (unground, _) = theme("text", "\"ui.background\" = { fg = \"#cdd6f4\" }\n");
        assert_eq!(
            unground.cursor(),
            None,
            "text on the terminal's ground: the terminal's cursor suits it"
        );
        let (named, notes) = theme(
            "named",
            "\"ui.background\" = { fg = \"white\", bg = \"#fdf6e3\" }\n",
        );
        assert_eq!(named.cursor(), None);
        assert!(
            notes
                .iter()
                .any(|n| n.contains("ui.cursor") && n.contains("#rrggbb")),
            "a ground whose text has no value is said: {notes:?}"
        );
    }

    /// **Every file in `themes/` is shipped, and every shipped name is a
    /// file there** — counted from the directory, which a list cannot do.
    #[test]
    fn every_theme_file_is_shipped() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("themes");
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .expect("themes/ reads")
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                name.strip_suffix(".toml").map(str::to_string)
            })
            .collect();
        files.sort();
        let mut shipped: Vec<String> = SHIPPED.iter().map(|(n, _)| n.to_string()).collect();
        shipped.sort();
        assert_eq!(files, shipped);
    }

    /// **Yours before the shipped one** — a file of yours named for a shipped
    /// theme is yours — and a theme of yours may inherit a shipped one.
    #[test]
    fn a_users_theme_is_theirs_and_may_inherit_a_shipped_one() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("nord.toml"), "keyword = \"#010203\"\n").unwrap();
        let (mine, _) = Theme::load("nord", Some(dir.path())).unwrap();
        assert_eq!(mine.fg(Ink::Keyword), Color::Rgb(1, 2, 3));
        assert_eq!(
            mine.ui(Ink::Background),
            Style::default(),
            "not the shipped nord"
        );
        std::fs::write(
            dir.path().join("mocha-mine.toml"),
            "inherits = \"catppuccin-mocha\"\nkeyword = \"#010203\"\n",
        )
        .unwrap();
        let (child, notes) = Theme::load("mocha-mine", Some(dir.path())).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(child.fg(Ink::Keyword), Color::Rgb(1, 2, 3));
        assert_eq!(
            child.ui(Ink::Background),
            Theme::load("catppuccin-mocha", None)
                .unwrap()
                .0
                .ui(Ink::Background),
            "the rest is the shipped mocha"
        );
    }

    /// **A terminal colour named in `[palette]` is that colour for every
    /// role** — the inherited ones, painted `magenta` by the shipped theme,
    /// included; written as a number too; and a slot it does not name stays
    /// the terminal's. So sixteen entries make a theme of a terminal palette.
    #[test]
    fn a_terminal_colour_named_in_the_palette_is_that_colour_everywhere() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mocha.toml"),
            "\"ui.question\" = 5\n[palette]\nmagenta = \"#cba6f7\"\nbright-black = \"#585b70\"\n",
        )
        .unwrap();
        let (theme, notes) = Theme::load("mocha", Some(dir.path())).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        let mauve = Color::Rgb(0xcb, 0xa6, 0xf7);
        let code = |ink| theme.paint(Context::Code, ink).fg;
        assert_eq!(
            code(Ink::Keyword),
            Some(mauve),
            "the shipped theme's magenta"
        );
        assert_eq!(code(Ink::Operand), Some(mauve), "a UI role's too");
        assert_eq!(code(Ink::Question), Some(mauve), "and `5`, the same slot");
        assert_eq!(code(Ink::Comment), Some(Color::Rgb(0x58, 0x5b, 0x70)));
        assert_eq!(
            code(Ink::String),
            Some(Color::Green),
            "not named: the terminal's"
        );
        assert_eq!(theme.ansi(5), mauve);
        assert_eq!(theme.ansi(2), Color::Green);
        assert_eq!(theme.said(Color::Magenta), mauve);
        // A file inheriting it keeps its palette, and may say a slot again.
        std::fs::write(
            dir.path().join("child.toml"),
            "inherits = \"mocha\"\n[palette]\ngreen = \"#a6e3a1\"\n",
        )
        .unwrap();
        let (child, _) = Theme::load("child", Some(dir.path())).unwrap();
        assert_eq!(child.paint(Context::Code, Ink::Keyword).fg, Some(mauve));
        assert_eq!(
            child.paint(Context::Code, Ink::String).fg,
            Some(Color::Rgb(0xa6, 0xe3, 0xa1))
        );
    }

    /// **A table gives a background**, and `ui.background` is read as one;
    /// a role's own key beats a prefix written beside it — `ui.border` does
    /// not reach the focused border, which the shipped theme names.
    #[test]
    fn a_role_has_a_background_and_its_own_key_wins() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("ground.toml"),
            "\"ui.background\" = { fg = \"#cdd6f4\", bg = \"#1e1e2e\" }\n\"ui.border\" = \"red\"\n",
        )
        .unwrap();
        let (theme, notes) = Theme::load("ground", Some(dir.path())).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        let ground = theme.ui(Ink::Background);
        assert_eq!(ground.fg, Some(Color::Rgb(0xcd, 0xd6, 0xf4)));
        assert_eq!(ground.bg, Some(Color::Rgb(0x1e, 0x1e, 0x2e)));
        assert_eq!(theme.fg(Ink::Border), Color::Red);
        assert_eq!(
            theme.fg(Ink::BorderFocused),
            Theme::terminal().fg(Ink::BorderFocused),
            "the shipped theme names it, and a more specific key wins"
        );
        // Unset, the terminal's own: nothing painted.
        assert_eq!(Theme::terminal().ui(Ink::Background), Style::default());
    }

    /// **Two verdict colours an eye would not tell apart** — the same, one
    /// terminal hue in two shades, or 24-bit colours a few steps apart; and
    /// not colours plainly apart, nor one left to the terminal.
    #[test]
    fn verdict_colours_alike_are_told_apart_by_eye() {
        assert!(alike(Some(Color::Red), Some(Color::Red)));
        assert!(alike(Some(Color::Red), Some(Color::LightRed)));
        assert!(alike(
            Some(Color::Rgb(0xff, 0x88, 0x00)),
            Some(Color::Rgb(0xff, 0x8a, 0x02))
        ));
        assert!(!alike(Some(Color::Red), Some(Color::Green)));
        assert!(!alike(
            Some(Color::Rgb(0xf3, 0x8b, 0xa8)),
            Some(Color::Rgb(0xa6, 0xe3, 0xa1))
        ));
        assert!(!alike(None, None), "the terminal's to keep apart");
        assert!(Theme::terminal().verdicts_alike().is_empty());
    }

    /// The text's own emphasis adds to the role's.
    #[test]
    fn a_cells_emphasis_adds_to_its_roles() {
        let theme = Theme::terminal();
        let style = theme.style(Context::Code, CellStyle::ink(Ink::Comment).bold());
        assert_eq!(style.fg, Some(Color::DarkGray));
        assert!(
            style
                .add_modifier
                .contains(Modifier::BOLD | Modifier::ITALIC)
        );
        assert_eq!(
            theme.style(Context::Code, CellStyle::PLAIN),
            Style::default()
        );
    }

    /// WCAG's contrast ratio of two 24-bit colours, 1 (alike) to 21.
    fn contrast(a: Color, b: Color) -> Option<f64> {
        let luminance = |c: Color| {
            let Color::Rgb(r, g, b) = c else { return None };
            let lin = |v: u8| {
                let s = f64::from(v) / 255.0;
                if s <= 0.04045 {
                    s / 12.92
                } else {
                    ((s + 0.055) / 1.055).powf(2.4)
                }
            };
            Some(0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b))
        };
        let (a, b) = (luminance(a)?, luminance(b)?);
        Some((a.max(b) + 0.05) / (a.min(b) + 0.05))
    }

    /// **Every role a theme paints can be read on its ground** — 1.5:1 at
    /// least, below the faintest colour a theme's author chose (tokyonight
    /// storm's comments, 1.63:1) and above `terminal`'s grey inherited by a
    /// light theme whose ground it nearly is (Solarized light's thinking and
    /// tab line, 1.14:1, unseen until 11-01). Every shipped theme with a
    /// ground, and a light ghostty palette imported, in both contexts.
    #[test]
    fn every_role_can_be_read_on_its_ground() {
        const FLOOR: f64 = 1.5;
        const SOLARIZED_LIGHT: [&str; 16] = [
            "073642", "dc322f", "859900", "b58900", "268bd2", "d33682", "2aa198", "eee8d5",
            "002b36", "cb4b16", "586e75", "657b83", "839496", "6c71c4", "93a1a1", "fdf6e3",
        ];
        let dir = tempfile::TempDir::new().unwrap();
        let mut ghostty = String::from("background = fdf6e3\nforeground = 657b83\n");
        for (slot, hex) in SOLARIZED_LIGHT.iter().enumerate() {
            ghostty.push_str(&format!("palette = {slot}=#{hex}\n"));
        }
        std::fs::write(dir.path().join("Paper Light"), ghostty).unwrap();
        let places = Places {
            yours: None,
            imported: vec![dir.path().to_path_buf()],
        };
        let mut themes: Vec<(String, Theme)> = shipped_names()
            .map(|name| (name.to_string(), Theme::load(name, None).unwrap().0))
            .collect();
        themes.push((
            "Paper Light (ghostty)".into(),
            Theme::load_from("Paper Light", &places).unwrap().0,
        ));
        let mut grounded = 0;
        let mut unread = Vec::new();
        for (name, theme) in &themes {
            let Some(ground) = theme.ui(Ink::Background).bg else {
                continue;
            };
            grounded += 1;
            for &ink in Ink::ALL {
                for context in [Context::Code, Context::Decision] {
                    let paint = theme.paint(context, ink);
                    let Some(fg) = paint.fg else { continue };
                    if let Some(ratio) = contrast(fg, paint.bg.unwrap_or(ground))
                        && ratio < FLOOR
                    {
                        unread.push(format!("{name}: {context:?} {} {ratio:.2}", ink.name()));
                    }
                }
            }
            // The cursor's block on the ground it moves over — what image 42
            // could not show (Steven, 11-02).
            match theme
                .cursor()
                .map(|(r, g, b)| contrast(Color::Rgb(r, g, b), ground))
            {
                Some(Some(ratio)) if ratio >= FLOOR => {}
                other => unread.push(format!("{name}: the cursor {other:?}")),
            }
        }
        assert_eq!(
            grounded,
            themes.len() - 1,
            "premise: every theme but `terminal` names its ground, the import too"
        );
        assert!(unread.is_empty(), "{unread:#?}");
    }

    /// **A thought is quiet text until a theme names it** — left out of
    /// the shipped table, as the rule around your message is, so a light
    /// theme that repaints `ui.text.quiet` draws thinking in it too.
    #[test]
    fn thinking_is_quiet_text_unless_named() {
        let terminal = Theme::terminal();
        assert_eq!(terminal.ui(Ink::Thinking), terminal.ui(Ink::TextQuiet));
        let (light, _) = Theme::load("solarized-light", None).unwrap();
        assert_ne!(
            light.fg(Ink::TextQuiet),
            light.said(terminal.fg(Ink::TextQuiet)),
            "premise: Solarized light repaints quiet text"
        );
        assert_eq!(light.ui(Ink::Thinking), light.ui(Ink::TextQuiet));
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("thought.toml"),
            "inherits = \"terminal\"\n\"ui.text.quiet.thinking\" = \"green\"\n",
        )
        .unwrap();
        let (named, _) = Theme::load("thought", Some(dir.path())).unwrap();
        assert_eq!(named.fg(Ink::Thinking), Color::Green);
        assert_eq!(named.fg(Ink::TextQuiet), terminal.fg(Ink::TextQuiet));
    }
}
