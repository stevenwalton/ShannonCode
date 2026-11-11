//! **`shannon tutor`** — Shannon's lessons, vimtutor's way
//! (`docs/DesignDocs/tutor.md`): a page you may change
//! (`page::Recipe::Tutor`, edited in place and never written), the stub
//! answering, in a directory of its own, on Shannon's defaults — vimtutor
//! runs vim with `-u NONE` so its lessons describe the vim you get, and a
//! config read here could also put a real model back behind the lessons'
//! "send this" (`:reload` lays your file over the tutor's settings).
//!
//! Steven's draft, in his voice, is the source of the text (`tutor.txt`).

use std::path::{Path, PathBuf};

use ratatui::style::Style;

use crate::help::{Dialect, Mark};
use crate::paint::{PageLine, Run};
use crate::style::{CellStyle, Ink};

/// The lessons as written, `{VERSION}` filled — no trailing newline, so the
/// text and its lines ([`lines`]) count alike. Its pictures are markers
/// still ([`DECISION`]); [`drawn_in`] draws them in.
pub(crate) fn text() -> String {
    include_str!("tutor.txt")
        .trim_end_matches('\n')
        .replace("{VERSION}", env!("CARGO_PKG_VERSION"))
}

/// **The lessons' width**, the `====` banner's — what a picture is drawn at.
/// 78: an 80-column terminal's pane is two narrower inside its border, and at
/// 80 the banner and the picture's frame wrapped (review stop 17, F M7).
pub(crate) const WIDTH: u16 = 78;

/// **Where a lesson pictures the decision pane**: a line holding only this,
/// below the command it pictures — the lesson's ```` ```tool bash ```` block
/// before it ([`Pictures::commands`]).
pub(crate) const DECISION: &str = "{DECISION}";

/// **What the lessons picture, drawn by what they picture** — each
/// [`DECISION`] the pane the command above it asks, drawn by the decision
/// pane itself (`App::tutor_pictures`, through `view::decision_picture`)
/// under the session's own policy, theme and breaks. Typed by hand, the
/// picture was what Steven could type of it (11-08: "We should be rendering
/// just as shannon's would… Make sure it paints accurately!"): `aw k` for
/// `awk`, a legend's keys unpainted, a verdict line out of date as soon as
/// the rules changed.
///
/// Carried by the page's recipe ([`Lessons`]), so the page paints a drawn
/// row as it was drawn whenever its text is painted again — every edit
/// ([`lines`]); a row you change is yours, and plain.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Pictures {
    /// Each marker's rows, in the text's order.
    drawn: Vec<Vec<PageLine>>,
}

impl Pictures {
    /// The pictures drawn, one per marker, in the text's order.
    pub(crate) fn of(drawn: Vec<Vec<PageLine>>) -> Pictures {
        Pictures { drawn }
    }

    /// **The command each marker in `text` pictures**, in order: the body
    /// of the last ```` ```tool bash ```` block before it, its lines joined
    /// — empty for a marker with none above it, which is drawn as nothing.
    pub(crate) fn commands(text: &str) -> Vec<String> {
        markers(text)
            .into_iter()
            .map(|(_, command)| command)
            .collect()
    }

    /// Each drawn row by its text, the first of any the same — a row is the
    /// pane's whole width, so a line of prose is never one. Built once per
    /// painting: asked row by row it was most of what an edit cost (stop 17,
    /// B F6).
    fn rows(&self) -> std::collections::HashMap<String, &PageLine> {
        let mut rows = std::collections::HashMap::new();
        for row in self.drawn.iter().flatten() {
            rows.entry(row.text()).or_insert(row);
        }
        rows
    }

    /// **`fresh`'s colours, on the pictures they are the same text as** — a
    /// picture whose words moved since (a rule of yours changed its verdict)
    /// keeps the rows the page's text holds, and its colours with them.
    pub(crate) fn recolour(&mut self, fresh: Pictures) {
        let same = |a: &[PageLine], b: &[PageLine]| {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.text() == b.text())
        };
        for (held, new) in self.drawn.iter_mut().zip(fresh.drawn) {
            if same(held, &new) {
                *held = new;
            }
        }
    }
}

/// **Where `text` holds a [`DECISION`] marker**, each line's index and the
/// command it pictures — the one reading, for what to draw and where to put
/// it (a marker inside a ```` ```tool bash ```` block is that block's text).
fn markers(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut last = String::new();
    let mut open: Option<Vec<&str>> = None;
    for (at, line) in text.split('\n').enumerate() {
        let trimmed = line.trim();
        if let Some(body) = &mut open {
            match trimmed == "```" {
                true => {
                    last = body.join("\n");
                    open = None;
                }
                false => body.push(line),
            }
        } else if trimmed == "```tool bash" {
            open = Some(Vec::new());
        } else if trimmed == DECISION {
            out.push((at, last.clone()));
        }
    }
    out
}

/// **What a tutor page is made of** (`page::Recipe::Tutor`): the lessons'
/// text, the shipped [`text`], and the pictures drawn for it — one value,
/// so a page and whatever redraws its pictures read the same text.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Lessons {
    pub(crate) text: String,
    pub(crate) pictures: Pictures,
}

/// **`text` with its pictures drawn in** — each [`DECISION`] line replaced
/// by its picture's rows (`pictures`, in order); a marker with no picture
/// stays as written.
pub(crate) fn drawn_in(text: &str, pictures: &Pictures) -> String {
    let at: std::collections::HashMap<usize, &Vec<PageLine>> = (markers(text)
        .into_iter()
        .map(|(at, _)| at)
        .zip(&pictures.drawn))
    .collect();
    let lines: Vec<String> = (text.split('\n').enumerate())
        .flat_map(
            |(n, line)| match at.get(&n).filter(|rows| !rows.is_empty()) {
                Some(rows) => rows.iter().map(PageLine::text).collect(),
                None => vec![line.to_string()],
            },
        )
        .collect();
    lines.join("\n")
}

/// **The lessons painted** — one line per `'\n'`-separated line of `text`,
/// exactly: an edit's text is its buffer's lines joined by `'\n'`
/// (`TextView::text`), and `str::lines` would drop a last empty line and a
/// `'\r'`, so the painted page and the text would disagree on what line the
/// cursor is on. A row of a picture (`pictures`) is painted as it was drawn.
pub(crate) fn lines(text: &str, pictures: &Pictures) -> Vec<PageLine> {
    let rows: Vec<&str> = text.split('\n').collect();
    let pictured = pictures.rows();
    let drawn: Vec<Option<&PageLine>> = (rows.iter())
        .map(|row| pictured.get(*row).copied())
        .collect();
    let mut fenced = false;
    let kinds: Vec<Kind> = (rows.iter())
        .map(|row| {
            let fence = row.trim_start().starts_with("```");
            let code = fenced || fence;
            fenced ^= fence;
            kind(row, code)
        })
        .collect();
    let prose: Vec<bool> = (0..rows.len())
        .map(|n| kinds[n] == Kind::Prose && !rows[n].trim().is_empty())
        .collect();
    let marks = prose_marks(&rows, &prose);
    let graphs = graph_drawings(&rows, &kinds, &drawn);
    (rows.iter().zip(kinds).zip(drawn).zip(marks).zip(graphs))
        .map(
            |((((row, kind), drawn), marks), graph)| match (drawn, graph) {
                (Some(drawn), _) => drawn.clone(),
                (None, Some(graph)) => graph,
                (None, None) => line(row, kind, marks),
            },
        )
        .collect()
}

/// **Each drawing of `:graph`, painted** — a run of drawing rows whose box is
/// titled `graph` ([`graph_drawing`]); every other row `None`, and every other
/// drawing plain, as a picture of another program's screen is.
fn graph_drawings(
    rows: &[&str],
    kinds: &[Kind],
    drawn: &[Option<&PageLine>],
) -> Vec<Option<PageLine>> {
    let mut out = vec![None; rows.len()];
    let mut n = 0;
    while n < rows.len() {
        let run = (n..rows.len())
            .take_while(|&m| kinds[m] == Kind::Drawing && drawn[m].is_none())
            .count();
        if run > 0 && rows[n].trim_start().starts_with("┌ graph ") {
            for (at, painted) in graph_drawing(&rows[n..n + run]).into_iter().enumerate() {
                out[n + at] = Some(painted);
            }
        }
        n += run.max(1);
    }
    out
}

/// What a tree draws with — `:graph`'s glyphs, its lanes' and its arrows'.
const TREE: &[char] = &['●', '○', '│', '├', '╰', '╭', '─', '┼', '┤', '╯', '╮'];

/// **A drawing of `:graph`, painted as `:graph` paints** — approximately,
/// by the shapes it draws (Steven, 11-09: "I really just want the colors to
/// reasonably look like the real `:graph`"; nothing loaded): each pictured
/// pane's border, the first, the graph, as the focused one; the tree's glyphs
/// in lane colours, two columns a lane, as a branch steps right; a session's
/// id and its `(…)` dim, a message's `you:` and `reply:` dim, the `HEAD` row
/// bold (`view::git_label_spans`); beside it the inspector's heading bold,
/// its keys dim and a `~` past the end (`view::inspector_lines`).
fn graph_drawing(rows: &[&str]) -> Vec<PageLine> {
    let cells: Vec<Vec<char>> = rows.iter().map(|row| row.chars().collect()).collect();
    let top = cells.first().cloned().unwrap_or_default();
    let mut panes: Vec<(usize, usize, String)> = Vec::new();
    let mut at = 0;
    while let Some(left) = (at..top.len()).find(|&c| top[c] == '┌') {
        let Some(right) = (left..top.len()).find(|&c| top[c] == '┐') else {
            break;
        };
        let title: String = top[left + 1..right].iter().collect();
        let title =
            title.trim_matches(|c: char| c == '─' || c.is_whitespace() || c.is_ascii_digit());
        panes.push((left, right, title.to_string()));
        at = right + 1;
    }
    let last = cells.len().saturating_sub(1);
    let inside = |r: usize| r > 0 && r < last;
    // Each tree's leftmost glyph, which its lanes count from — in its own
    // pane, from its body's first column. One was taken from the first pane
    // titled `graph` for every such pane, so a second (the page is yours to
    // retitle) counted from another's columns, and `f - left - 1`
    // underflowed (stop 17, V2b M2).
    let lanes_from: Vec<Option<usize>> = (panes.iter())
        .map(|(left, right, title)| {
            let body = left + 1..*right;
            (title == "graph")
                .then(|| {
                    (0..cells.len())
                        .filter(|&r| inside(r))
                        .filter_map(|r| {
                            (body.start..body.end.min(cells[r].len()))
                                .find(|&c| TREE.contains(&cells[r][c]))
                        })
                        .min()
                })
                .flatten()
                .map(|c| c - body.start)
        })
        .collect();
    // The inspector's first words are its heading.
    let heading =
        (panes.iter().find(|(_, _, title)| title == "inspect")).and_then(|&(left, right, _)| {
            (0..cells.len()).find(|&r| {
                inside(r)
                    && (cells[r].get(left + 1..right))
                        .is_some_and(|s| s.iter().any(|c| !c.is_whitespace()))
            })
        });
    (cells.iter().enumerate())
        .map(|(r, row)| {
            let mut styles = vec![CellStyle::PLAIN; row.len()];
            // The page is yours to change: a row cut short, or emptied, is
            // painted as far as it goes.
            let Some(end) = row.len().checked_sub(1) else {
                return PageLine::of_cells("", &styles);
            };
            for (n, (left, right, title)) in panes.iter().enumerate() {
                let border = CellStyle::ink(match n {
                    0 => Ink::BorderFocused,
                    _ => Ink::Border,
                });
                let right = (*right).min(end);
                if *left > right {
                    continue;
                }
                for c in *left..=right {
                    let edge = c == *left || c == right;
                    let framing = !inside(r) && (is_box(row[c]) || row[c].is_ascii_digit());
                    if edge || framing {
                        styles[c] = border;
                    }
                }
                if !inside(r) {
                    continue;
                }
                let body = (left + 1).min(right)..right;
                match title.as_str() {
                    "graph" => graph_row(&row[body.clone()], lanes_from[n], &mut styles[body]),
                    "inspect" => {
                        inspector_row(&row[body.clone()], heading == Some(r), &mut styles[body])
                    }
                    _ => {}
                }
            }
            PageLine::of_cells(&row.iter().collect::<String>(), &styles)
        })
        .collect()
}

/// A box's edge (U+2500..U+257F).
fn is_box(c: char) -> bool {
    ('\u{2500}'..='\u{257f}').contains(&c)
}

/// One row inside a pictured `:graph`: its glyphs in their lanes, counted
/// from `lanes_from`; its label after them, as `view::git_label_spans` paints
/// one.
fn graph_row(cells: &[char], lanes_from: Option<usize>, styles: &mut [CellStyle]) {
    let from = lanes_from.unwrap_or(0);
    let mut last_glyph = None;
    for (c, cell) in cells.iter().enumerate() {
        if TREE.contains(cell) {
            styles[c] = CellStyle::ink(Ink::LANES[(c.saturating_sub(from) / 2) % Ink::LANES.len()]);
            last_glyph = Some(c);
        }
    }
    let Some(glyph) = last_glyph else {
        return;
    };
    let Some(start) = (glyph + 1..cells.len()).find(|&c| !cells[c].is_whitespace()) else {
        return;
    };
    let label: String = cells[start..].iter().collect();
    let mut dim_from = |at: usize| {
        for style in &mut styles[at..] {
            *style = style.dim();
        }
    };
    if label.starts_with("you:") || label.starts_with("reply:") {
        dim_from(start);
    } else if label.contains("  (") {
        // A session: its name (else its id) leads; the rest is dim.
        let word = (start..cells.len()).find(|&c| cells[c].is_whitespace());
        if let Some(word) = word {
            dim_from(word);
        }
    } else if let Some(more) = cells.iter().rposition(|&c| c == '⋯') {
        styles[more] = styles[more].dim();
    }
    if label.contains("HEAD") {
        for style in styles.iter_mut() {
            *style = style.bold();
        }
    }
}

/// One row inside a pictured inspector: its `heading` bold, a field's key
/// (right-aligned, before two spaces) dim, a `~` past the end.
fn inspector_row(cells: &[char], heading: bool, styles: &mut [CellStyle]) {
    let text: String = cells.iter().collect();
    if text.trim() == "~" {
        if let Some(at) = cells.iter().position(|&c| c == '~') {
            styles[at] = CellStyle::ink(Ink::NonText);
        }
    } else if heading {
        for (style, cell) in styles.iter_mut().zip(cells) {
            if !cell.is_whitespace() {
                *style = style.bold();
            }
        }
    } else if text.starts_with(' ')
        && let Some(gap) = text.trim_start().find("  ")
    {
        let key = text.len() - text.trim_start().len() + gap;
        for style in &mut styles[..text[..key].chars().count()] {
            *style = style.dim();
        }
    }
}

/// **The marks of each prose line** (`prose`), read a paragraph at a time —
/// a run of prose lines, which a blank line or anything that is no prose
/// ends — since emphasis crosses a line break, as markdown's does, and each
/// clipped to its line.
fn prose_marks(rows: &[&str], prose: &[bool]) -> Vec<Vec<(std::ops::Range<usize>, Mark)>> {
    let mut out = vec![Vec::new(); rows.len()];
    let mut n = 0;
    while n < rows.len() {
        let end = n + prose[n..].iter().take_while(|is| **is).count();
        if end == n {
            n += 1;
            continue;
        }
        let paragraph = rows[n..end].join("\n");
        // Marks come in order, so the row the next one starts on only moves
        // on: from the paragraph's top each time, a long list stalled every
        // keystroke (review stop 17, B F3).
        let (mut first, mut first_start) = (n, 0);
        for (at, mark) in crate::help::marks(&paragraph, Dialect::Tutor) {
            while first_start + rows[first].len() < at.start {
                first_start += rows[first].len() + 1;
                first += 1;
            }
            let (mut row, mut start) = (first, first_start);
            while row < end && start < at.end {
                let on_row = at.start.max(start)..at.end.min(start + rows[row].len());
                if !on_row.is_empty() {
                    out[row].push((on_row.start - start..on_row.end - start, mark));
                }
                start += rows[row].len() + 1;
                row += 1;
            }
        }
        n = end;
    }
    out
}

/// **What a line of the lessons is** — the one reading, for how it is
/// painted ([`line`]) and for where a paragraph ends ([`prose_marks`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// In a code block, its fences too.
    Code,
    /// A box's edge (U+2500..U+257F): what it holds is a picture.
    Drawing,
    /// `====…`, `~~~~…`.
    Rule,
    /// A lesson's title, the banner, or a centred line in capitals ("THAT'S
    /// THE TOUR").
    Title,
    /// `---> …`, a line you are meant to fix.
    Practice,
    Prose,
}

fn kind(text: &str, code: bool) -> Kind {
    let trimmed = text.trim_start();
    let centred_caps = text.starts_with("        ")
        && trimmed.chars().any(|c| c.is_ascii_uppercase())
        && !trimmed.chars().any(char::is_lowercase);
    if code {
        Kind::Code
    } else if trimmed.starts_with(|c: char| ('\u{2500}'..='\u{257f}').contains(&c)) {
        Kind::Drawing
    } else if trimmed.starts_with("~~~") || is_rule(trimmed) {
        Kind::Rule
    } else if is_title(trimmed) || centred_caps {
        Kind::Title
    } else if text.starts_with("--->") {
        Kind::Practice
    } else {
        Kind::Prose
    }
}

/// One line, by its [`Kind`]: a code block's whole, as code — a command's
/// `|` and `*` are not the manual's links and tags; a drawing's whole and
/// plain, for the same reason; a rule faint, a title a heading, a practice
/// line's `--->` marked; prose by `marks` (`help_view::style_marked`:
/// `` `literals` ``, `|links|`, keys, and `*emphasis*`, since the tutor
/// defines no tags). The manual's own rule would read `--->` as a `---`
/// rule and fade the line you are meant to fix.
///
/// **What a lesson paints is what it marks**: a compass's `^` and `v` are
/// shaped like a table's keys, and a lesson quotes the keys it means
/// (Steven, 11-08), so no key is guessed from a row's shape here.
fn line(text: &str, kind: Kind, marks: Vec<(std::ops::Range<usize>, Mark)>) -> PageLine {
    let whole = |look: CellStyle| PageLine {
        runs: vec![Run::role(text, look)],
        style: Style::default(),
    };
    match kind {
        Kind::Code => whole(CellStyle::ink(Ink::HelpCode)),
        Kind::Drawing => whole(CellStyle::PLAIN),
        Kind::Rule => whole(CellStyle::ink(Ink::TextFaint)),
        Kind::Title => whole(CellStyle::ink(Ink::HelpHeading).bold()),
        Kind::Practice => {
            let rest = &text["--->".len()..];
            let mut runs = vec![Run::role("--->", CellStyle::ink(Ink::HelpTag).bold())];
            let marks = crate::help::marks(rest, Dialect::Tutor);
            runs.extend(crate::help_view::style_marked(rest, marks, None, CellStyle::PLAIN).runs);
            PageLine {
                runs,
                style: Style::default(),
            }
        }
        Kind::Prose => crate::help_view::style_marked(text, marks, None, CellStyle::PLAIN),
    }
}

/// `====…`, nothing else on it.
fn is_rule(trimmed: &str) -> bool {
    !trimmed.is_empty() && trimmed.chars().all(|c| c == '=')
}

/// A lesson's title (`Lesson 1.2: …`), or the banner's `= Welcome … =`.
fn is_title(trimmed: &str) -> bool {
    let lesson = trimmed
        .strip_prefix("Lesson ")
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
    let banner = trimmed.len() > 2
        && trimmed.starts_with("= ")
        && trimmed.ends_with(" =")
        && trimmed.chars().any(char::is_alphabetic);
    lesson || banner
}

/// What the tutor's directory holds when it starts — what the lessons send
/// the stub to read, change and delete. A copy, as vimtutor's text is.
const FILES: &[(&str, &str)] = &[(
    "notes.txt",
    "Shannon tutor notes\n\
     \n\
     This file is the tutor's own copy: read it, change it, delete it.\n\
     Nothing outside this directory is touched by the lessons.\n",
)];

/// **A directory of the tutor's own**, made under `parent` (the system's
/// temporary directory) and seeded with `FILES`: an approved call in a
/// lesson runs THERE, and touches nothing of yours. Owner-only, as every
/// directory Shannon makes; named by the process and the moment, and made
/// fresh — never one that already exists, which someone else may have put
/// there (each file is created the same way).
pub fn prepare_dir(parent: &Path) -> std::io::Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let dir = parent.join(format!("shannon-tutor-{}-{stamp:x}", std::process::id()));
    make(&dir)?;
    Ok(dir)
}

/// `dir` made owner-only and [`seed`]ed — refused when it already exists.
fn make(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(dir)?;
    seed(dir)
}

/// `FILES` written into `dir`, each owner-only — refused, the one there
/// untouched, when a file of that name is already in it.
fn seed(dir: &Path) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    for (name, body) in FILES {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dir.join(name))?
            .write_all(body.as_bytes())?;
    }
    Ok(())
}

/// **The tutor's own settings**, laid over the defaults as the tutor's
/// layer (`ConfigStack::start_tutor`): the stub answers every session it
/// makes (`:model` still reaches a real one), and the prompt is 3 rows, so
/// at 24 rows the lessons keep the screen (`tutor.md` T4).
///
/// **And no project above its directory** (`[memory] project_root = []`):
/// the walk for memory, skills and `AGENTS.md` climbed from the tutor's
/// directory to the nearest ancestor holding a `.git` — under the system's
/// temporary directory, which on Linux is `/tmp`, where any account can
/// make one — and granted reads of what it found, and read the
/// instructions whole into every session (review stop 16, E 4, measured).
/// With no markers the walk is the directory alone.
pub fn settings() -> toml::Table {
    let mut prompt = toml::Table::new();
    prompt.insert("height".into(), 3.into());
    let mut tui = toml::Table::new();
    tui.insert("model".into(), "stub".into());
    tui.insert("prompt".into(), toml::Value::Table(prompt));
    let mut memory = toml::Table::new();
    memory.insert("project_root".into(), toml::Value::Array(Vec::new()));
    toml::Table::from_iter([
        ("tui".to_string(), toml::Value::Table(tui)),
        ("memory".to_string(), toml::Value::Table(memory)),
    ])
}

/// Why the tutor has no config file of yours to `:reload` or `:config edit`.
pub const READS_NO_CONFIG: &str =
    "the tutor runs on Shannon's defaults, as vimtutor does — your config is read outside it";

/// **What `shannon tutor` runs with, in place of what `shannon` does** — the
/// choices that would write your config, vault, trust or sessions, a value
/// here, not an arm of `main`, so a test holds it (review stop 16, C 1 and
/// C 2: deleting any of them passed every test, and two wrote your real
/// state — a practice `:secret` into your vault, a `:trust .` of the tutor's
/// directory into your trust file). Not among them, and yours as in any
/// launch: the log and audit trail, the key if you `:model` to a real one,
/// and what `:import` and `:themes` read (review stop 17, E2 4). `main`
/// takes them all at once (its `Chosen`), and still enters
/// `dir` itself, before anything reads where Shannon started; smoke holds
/// that. Shannon's own paths stay guarded as in any launch.
pub struct Launch {
    /// Its own directory ([`prepare_dir`]).
    pub dir: PathBuf,
    /// No file of yours: its settings ([`settings`]) over the defaults,
    /// under their own origin.
    pub stack: shannon_core::config::ConfigStack,
    /// What that stack adds up to.
    pub config: shannon_core::Config,
    /// Its chats: in memory, gone when you quit (`tutor.md` T2).
    pub store: Box<dyn shannon_core::Store>,
    /// Its secrets: none kept past the run — a practice `:secret` works,
    /// and says it "is not kept for later runs".
    pub vault: std::sync::Arc<dyn shannon_core::Vault>,
    /// Its trust: in memory — `:trust .` there trusts the tutor's directory
    /// for the run and writes nothing of yours.
    pub trusted: crate::trust::Trusted,
    /// The Claude Code sessions it starts, should a lesson send to Claude
    /// Code: in memory, so none joins your list.
    pub claude_sessions: crate::claude_sessions::ClaudeSessions,
}

impl Launch {
    /// The directory made under `parent`, the settings laid. Refused, the
    /// launch stops: a tutor whose stub did not take would answer its
    /// lessons from a real model.
    pub fn prepare(parent: &Path) -> anyhow::Result<Launch> {
        use anyhow::Context as _;
        let dir = prepare_dir(parent).context("make the tutor's directory")?;
        let mut stack = shannon_core::config::ConfigStack::default();
        let config = (stack.start_tutor(settings()))
            .map_err(|why| anyhow::anyhow!("the tutor's settings were refused: {why}"))?;
        Ok(Launch {
            dir,
            stack,
            config,
            store: Box::new(shannon_core::MemoryStore::new()),
            vault: std::sync::Arc::new(shannon_core::NoVault),
            trusted: crate::trust::Trusted::load(None).0,
            claude_sessions: crate::claude_sessions::ClaudeSessions::load(None).0,
        })
    }

    /// **Where memory and skills are read from besides its directory:
    /// nowhere** — not your config directory, not your home: the
    /// `(config_dir, home)` of `App::launch_knowledge`. From yours, every
    /// tutor run made `projects/<its directory>/memory` in your config tree,
    /// and let writes into it run unasked (review stop 16, D 1, measured).
    pub fn knowledge(&self) -> (Option<PathBuf>, Option<PathBuf>) {
        (None, None)
    }

    /// **Its scratch directory: none** — its own directory is where a
    /// lesson works. The one an ordinary launch keeps was made and approved
    /// for it too (D 1).
    pub fn scratch(&self) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The page and its text agree on every line** — what an edit relies
    /// on: a last empty line, a `'\r'` mid-line, an empty text (`ggdG`
    /// leaves one empty line, as the editor's buffer does).
    #[test]
    fn its_lines_are_its_texts_lines_exactly() {
        for text in ["", "a", "a\n", "a\n\nb", "x\ry", "---> fix me\n"] {
            let painted: Vec<String> = (lines(text, &Pictures::default()).iter())
                .map(PageLine::text)
                .collect();
            let split: Vec<&str> = text.split('\n').collect();
            assert_eq!(painted, split, "{text:?}");
        }
    }

    /// **The shipped lessons paint as their kinds** — the banner, a lesson's
    /// title and the closing title a heading, a rule faint, a practice
    /// line's arrow marked, a drawing plain (the `|` of a command quoted in
    /// it no link), a code block code: read off the text that ships, where
    /// the painter was tested only on strings made for it, and four of its
    /// rules could go with every test green (review stop 16, C 3).
    #[test]
    fn the_shipped_lessons_paint_as_their_kinds() {
        use crate::paint::Look;
        let text = text();
        let page = lines(&text, &Pictures::default());
        let rows: Vec<&str> = text.split('\n').collect();
        let at = |is: &dyn Fn(&str) -> bool| (rows.iter().position(|l| is(l))).expect("a line");
        let looks = |i: usize| page[i].runs.iter().map(|r| r.look).collect::<Vec<_>>();
        let heading = Look::Role(CellStyle::ink(Ink::HelpHeading).bold());
        let faint = Look::Role(CellStyle::ink(Ink::TextFaint));
        let code = Look::Role(CellStyle::ink(Ink::HelpCode));
        let plain = Look::Role(CellStyle::PLAIN);
        assert_eq!(
            looks(at(&|l| l.contains("Welcome to Shannon Tutor"))),
            [heading]
        );
        assert_eq!(looks(at(&|l| l.contains("Lesson 1.0:"))), [heading]);
        assert_eq!(looks(at(&|l| l.contains("THAT'S THE TOUR"))), [heading]);
        assert_eq!(looks(0), [faint], "the banner's rule");
        assert_eq!(looks(at(&|l| l.starts_with("~~~~"))), [faint]);
        let arrow = at(&|l| l.starts_with("---> There is some text"));
        assert_eq!(
            page[arrow].runs[0].look,
            Look::Role(CellStyle::ink(Ink::HelpTag).bold())
        );
        assert!(page[arrow].runs[1..].iter().all(|r| r.look != faint));
        let drawing = at(&|l| l.starts_with("│") && l.contains("| tee permtest"));
        assert_eq!(looks(drawing), [plain], "a drawing's `|` is no link");
        assert_eq!(looks(at(&|l| l.starts_with("```tool bash"))), [code]);
        assert_eq!(looks(at(&|l| l.starts_with("mkdir -p permtest"))), [code]);
    }

    /// **A drawing of `:graph` is painted as `:graph` paints** (Steven, 11-09:
    /// "I really just want the colors to reasonably look like the real
    /// `:graph`") — its panes' borders, the tree's lanes, a session's id and
    /// decoration dim, a message dim, the `HEAD` row bold; the inspector's
    /// heading bold, its keys dim, `~` past the end. Another drawing stays
    /// plain, and so does the shipped one's twin of another program.
    #[test]
    fn a_drawing_of_the_graph_is_painted_as_the_graph_is() {
        use crate::paint::Look;
        let rows = [
            "┌ graph ────────────────────────1─┐┌ inspect ────────────2─┐",
            "│  ● tour e946fb37  (root)        ││Message                │",
            "│  ╰● 39f303f1  What does main.rs ││  session  5904e40f    │",
            "│   │ you: What does main.rs do?  ││     turn  2/3         │",
            "│▶  ├● 5904e40f  (HEAD, clone ← 39││                       │",
            "│   │ ● 678954fc  And the loop?   ││And the loop?          │",
            "│   ● 678954fc  And the loop?  ⋯  ││~                      │",
            "└─────────────────────────────────┘└───────────────────────┘",
            "",
            "┌ prompt ──────────────3─┐",
            "│ YOUR PROMPT HERE       │",
            "└────────────────────────┘",
        ];
        let text = rows.join("\n");
        let page = lines(&text, &Pictures::default());
        let at = |r: usize, col: usize| -> Look {
            let mut from = 0;
            for run in &page[r].runs {
                let n = run.text.chars().count();
                if col < from + n {
                    return run.look;
                }
                from += n;
            }
            panic!("no column {col} in row {r}")
        };
        let look = |r: usize, word: &str| {
            at(
                r,
                rows[r][..rows[r].find(word).expect(word)].chars().count(),
            )
        };
        let role = |ink| Look::Role(CellStyle::ink(ink));
        let plain = Look::Role(CellStyle::PLAIN);
        let dim = Look::Role(CellStyle::PLAIN.dim());
        let bold = |style: CellStyle| Look::Role(style.bold());
        assert_eq!(
            look(0, "┌ graph"),
            role(Ink::BorderFocused),
            "the graph's pane"
        );
        assert_eq!(look(0, "┌ inspect"), role(Ink::Border), "the inspector's");
        assert_eq!(at(3, 0), role(Ink::BorderFocused), "a left edge");
        assert_eq!(look(1, "●"), role(Ink::LANES[0]), "the root's lane");
        assert_eq!(at(3, 4), role(Ink::LANES[0]), "its spine");
        assert_eq!(
            look(4, "●"),
            bold(CellStyle::ink(Ink::LANES[1])),
            "the clone's lane"
        );
        assert_eq!(
            look(5, "● 678954fc"),
            role(Ink::LANES[1]),
            "the clone's turn"
        );
        assert_eq!(look(1, "tour"), plain, "a session's name");
        assert_eq!(look(1, "e946fb37"), dim, "its id");
        assert_eq!(look(1, "(root)"), dim, "its decoration");
        assert_eq!(look(3, "you:"), dim, "a message");
        assert_eq!(look(4, "5904e40f"), bold(CellStyle::PLAIN), "HEAD's");
        assert_eq!(look(6, "678954fc"), plain, "a turn");
        assert_eq!(look(6, "⋯"), dim, "more");
        assert_eq!(look(1, "Message"), bold(CellStyle::PLAIN), "the heading");
        assert_eq!(look(2, "session"), dim, "a key");
        assert_eq!(look(2, "5904e40f"), plain, "its value");
        assert_eq!(look(5, "And the"), plain, "a message's words");
        assert_eq!(look(6, "~"), role(Ink::NonText));
        for line in &page[9..12] {
            let looks: Vec<Look> = line.runs.iter().map(|run| run.look).collect();
            assert_eq!(looks, [plain], "another drawing: {}", line.text());
        }
        let shipped = text_of_shipped_graph();
        assert!(shipped, "the lessons draw `:graph`, painted");

        // The page is yours to change: any row cut at any length paints, its
        // text as written.
        for r in 0..8 {
            for keep in 0..=rows[r].chars().count() {
                let mut cut = rows.map(str::to_string);
                cut[r] = rows[r].chars().take(keep).collect();
                let text = cut.join("\n");
                let page = lines(&text, &Pictures::default());
                assert_eq!(page[r].text(), cut[r], "row {r} cut at {keep}");
            }
        }
        // A row cut to nothing ends the drawing on the page, so the painter
        // is asked of one only directly: it paints nothing, and goes on.
        let painted = graph_drawing(&[rows[0], "", rows[1]]);
        assert_eq!(painted[1].text(), "", "an empty row");
        assert_eq!(painted[2].text(), rows[1], "and the next");
        // And retitled: two panes called `graph` each count their lanes
        // from their own glyphs. One count for both underflowed and panicked
        // a debug build (stop 17, V2b M2).
        let mut retitled = rows.map(str::to_string);
        retitled[0] = rows[0].replacen("inspect", "graph  ", 1);
        assert_ne!(retitled[0], rows[0], "premise: a second `graph`");
        let refs: Vec<&str> = retitled.iter().map(String::as_str).collect();
        let painted: Vec<String> = (graph_drawing(&refs).iter()).map(PageLine::text).collect();
        assert_eq!(painted, retitled, "painted, its text as written");
    }

    /// Whether the shipped lessons hold a drawing of `:graph`, its frame
    /// painted.
    fn text_of_shipped_graph() -> bool {
        use crate::paint::Look;
        let text = text();
        let page = lines(&text, &Pictures::default());
        (text.split('\n').zip(&page)).any(|(row, line)| {
            row.trim_start().starts_with("┌ graph ")
                && line.runs[0].look == Look::Role(CellStyle::ink(Ink::BorderFocused))
        })
    }

    /// **A picture is drawn where its marker is, for the command above it**,
    /// and painted as drawn — until you change a row, which is then yours
    /// and plain.
    #[test]
    fn a_picture_is_drawn_at_its_marker_and_painted_as_drawn() {
        let text = format!(
            "{DECISION}\n```tool bash\nls\n```\nsee\n{DECISION}\n```tool bash\nrm a\nrm b\n```\n{DECISION}"
        );
        assert_eq!(
            Pictures::commands(&text),
            ["", "ls", "rm a\nrm b"],
            "the block above each"
        );
        let drawn = Style::default().fg(ratatui::style::Color::Red);
        let row = |text: &str| PageLine {
            runs: vec![Run::own(text, drawn)],
            style: Style::default(),
        };
        let pictures = Pictures::of(vec![vec![], vec![row("┌ one ┐"), row("└─────┘")], vec![]]);
        let filled = drawn_in(&text, &pictures);
        let rows: Vec<&str> = filled.split('\n').collect();
        assert_eq!(rows[0], DECISION, "no picture: the marker stays");
        assert_eq!(rows[5..7], ["┌ one ┐", "└─────┘"], "drawn in its place");
        let page = lines(&filled, &pictures);
        assert_eq!(
            page[5].runs[0].look,
            crate::paint::Look::Own(drawn),
            "painted as drawn"
        );
        let edited = filled.replace("┌ one ┐", "┌ mine ┐");
        let page = lines(&edited, &pictures);
        assert_eq!(
            page[5].runs[0].look,
            crate::paint::Look::Role(CellStyle::PLAIN),
            "yours, plain"
        );
    }

    /// What each run of `text`'s painted line `n` looks like, keyed by its
    /// words.
    fn looks(text: &str, n: usize) -> Vec<(String, crate::paint::Look)> {
        (lines(text, &Pictures::default())[n].runs.iter())
            .map(|run| (run.text.clone(), run.look))
            .collect()
    }

    /// **A lesson paints what it quotes, and guesses no key from a row's
    /// shape**: the compass's arrows stay plain beside the letters it
    /// quotes, though `^` and `v` head rows shaped like a key table's.
    #[test]
    fn a_compass_paints_its_letters_and_not_its_arrows() {
        use crate::paint::Look;
        let code = Look::Role(CellStyle::ink(Ink::HelpCode));
        let plain = Look::Role(CellStyle::PLAIN);
        let text = "        ^         To move\n       `k`       up\n    <`h` `l`>     sides\n        v         Hint";
        assert_eq!(
            looks(text, 0),
            [(text[..text.find('\n').unwrap()].to_string(), plain)]
        );
        assert_eq!(looks(text, 1)[1], ("`k`".to_string(), code));
        let sides = looks(text, 2);
        assert_eq!(sides[0], ("    <".to_string(), plain));
        assert_eq!(sides[1], ("`h`".to_string(), code));
        assert_eq!(sides[3], ("`l`".to_string(), code));
        assert_eq!(sides[4].1, plain, "the arrow's `>`: {sides:?}");
        assert!(looks(text, 3).iter().all(|(_, look)| *look == plain), "`v`");
    }

    /// **Emphasis as markdown reads it**, either delimiter — and it may
    /// cross a line break; a paragraph's end, a code block, a word's inside
    /// and an escape are where it may not.
    #[test]
    fn emphasis_is_read_as_markdown_reads_it() {
        use crate::paint::Look;
        let italic = Look::Role(CellStyle::PLAIN.italic());
        let bold = Look::Role(CellStyle::PLAIN.bold());
        let plain = Look::Role(CellStyle::PLAIN);
        let one = |text: &str| looks(text, 0);
        assert_eq!(one("We _could_ go")[1], ("_could_".to_string(), italic));
        assert_eq!(one("We *could* go")[1], ("*could*".to_string(), italic));
        assert_eq!(one("a __bold__ b")[1], ("__bold__".to_string(), bold));
        assert_eq!(one("a _b_c_ d")[1], ("_b_c_".to_string(), italic), "one");
        for none in [
            "snake_case_name",
            "¯\\_(ツ)_/¯",
            "^__^",
            "* a star",
            "* a*",
            "a_ b_",
            "a_b c_ d",
        ] {
            assert_eq!(one(none), [(none.to_string(), plain)], "{none}");
        }
        assert_eq!(one("*a * b*")[0], ("*a * b*".to_string(), italic));
        // Only emphasis crosses a line: a stray backtick pairs with nothing
        // on the next line.
        let text = "a `b\nc` d";
        assert_eq!(looks(text, 0), [("a `b".to_string(), plain)], "a literal");
        // Across a line break, each line's part its own run.
        let text = "see *this\nand that*, then\n\nnot *here\n\nor there*";
        assert_eq!(looks(text, 0)[1], ("*this".to_string(), italic));
        assert_eq!(looks(text, 1)[0], ("and that*".to_string(), italic));
        assert_eq!(
            looks(text, 3),
            [("not *here".to_string(), plain)],
            "paragraph"
        );
        let text = "a *star\n```\nb*\n```";
        assert_eq!(
            looks(text, 0),
            [("a *star".to_string(), plain)],
            "a fence ends it"
        );
    }

    /// A practice line is not a rule: the manual's `---` would fade it.
    #[test]
    fn a_practice_line_keeps_its_text_bright() {
        let text = "---> There is some text missing from th";
        let line = line(text, kind(text, false), Vec::new());
        assert_eq!(line.runs[0].text, "--->");
        assert!(
            line.runs[1..]
                .iter()
                .all(|run| run.look != crate::paint::Look::Role(CellStyle::ink(Ink::TextFaint))),
            "{line:?}"
        );
    }

    /// **A code block is code**: its command's `|` and `*` are not links
    /// and tags, and its fences are its own; after it, prose is marked
    /// again.
    #[test]
    fn a_code_block_is_painted_whole_and_ends_at_its_fence() {
        let code = crate::paint::Look::Role(CellStyle::ink(Ink::HelpCode));
        let page = lines(
            "```tool bash\nls | wc -l > a*b\n```\nsee |stub|",
            &Pictures::default(),
        );
        for line in &page[..3] {
            assert_eq!(line.runs.len(), 1, "{line:?}");
            assert_eq!(line.runs[0].look, code);
        }
        assert!(page[3].runs.len() > 1, "prose marked again: {:?}", page[3]);
    }

    /// **A picture takes new colours only where its words are the same** —
    /// the page's text holds the rows drawn, and a row it does not hold is
    /// painted as nothing.
    #[test]
    fn a_picture_takes_new_colours_where_its_words_are_the_same() {
        use ratatui::style::Color::{Green, Red};
        let row = |text: &str, colour| PageLine {
            runs: vec![Run::own(text, Style::default().fg(colour))],
            style: Style::default(),
        };
        let mut held = Pictures::of(vec![vec![row("a", Red)], vec![row("b", Red)]]);
        held.recolour(Pictures::of(vec![
            vec![row("a", Green)],
            vec![row("changed", Green)],
        ]));
        let rows = held.rows();
        assert_eq!(rows.get("a"), Some(&&row("a", Green)), "the same words");
        assert_eq!(rows.get("b"), Some(&&row("b", Red)), "words that moved");
        assert_eq!(rows.get("changed"), None);
    }

    /// **A drawing holds no marks** — what is in a box is a picture of
    /// something else, whose `|stub|` is no link (review stop 17, C 6).
    #[test]
    fn a_drawing_holds_no_marks() {
        let text = "│ see |stub| and *this* │";
        assert_eq!(
            looks(text, 0),
            [(text.to_string(), crate::paint::Look::Role(CellStyle::PLAIN))]
        );
    }

    /// **A marker inside a command block is the block's text** — one reading
    /// of a marker, for what to draw and for where (review stop 17, A L6).
    #[test]
    fn a_marker_in_a_command_block_is_its_text() {
        let text = format!("```tool bash\n{DECISION}\n```\n{DECISION}");
        assert_eq!(Pictures::commands(&text), [DECISION]);
        let row = PageLine {
            runs: vec![Run::own("drawn", Style::default())],
            style: Style::default(),
        };
        assert_eq!(
            drawn_in(&text, &Pictures::of(vec![vec![row]])),
            format!("```tool bash\n{DECISION}\n```\ndrawn")
        );
    }

    /// **Painting a long paste stays linear** — every opener with no closer
    /// scanned to the paragraph's end and every mark walked the paragraph's
    /// rows, on every keystroke: 6 s for 20k `* item` lines in a release
    /// build (review stop 17, B F2, F3). The margin is measured: a debug
    /// build paints each of these in tens of milliseconds.
    #[test]
    fn painting_a_long_paragraph_is_linear() {
        for unit in ["* item\n", "*a ", "**a ", "_a ", "`a`\n", "<", "CTRL-<"] {
            let text = unit.repeat(20_000);
            let began = std::time::Instant::now();
            let page = lines(&text, &Pictures::default());
            let took = began.elapsed();
            assert_eq!(page.len(), text.split('\n').count(), "{unit:?}");
            assert!(took.as_secs() < 3, "{unit:?}: {took:?}");
        }
    }

    proptest::proptest! {
        /// **Any text paints as itself**, and its marks are ordered, on
        /// characters' edges, and inside it — what a paste into the page can
        /// be (review stop 17, B).
        #[test]
        fn any_text_paints_as_itself(
            text in "(CTRL-|\\*\\*|__|[*_`|<>{}Cé ツ\\\\\n\ra-z-]){0,120}"
        ) {
            let painted: Vec<String> = (lines(&text, &Pictures::default()).iter())
                .map(PageLine::text)
                .collect();
            let split: Vec<&str> = text.split('\n').collect();
            proptest::prop_assert_eq!(painted, split);
            for dialect in [Dialect::Manual, Dialect::Tutor] {
                let mut end = 0;
                for (at, _) in crate::help::marks(&text, dialect) {
                    proptest::prop_assert!(end <= at.start && at.start < at.end);
                    proptest::prop_assert!(at.end <= text.len());
                    proptest::prop_assert!(
                        text.is_char_boundary(at.start) && text.is_char_boundary(at.end)
                    );
                    end = at.end;
                }
            }
        }
    }

    /// Its titles stand out; prose and a rule do not read as titles.
    #[test]
    fn a_lesson_title_is_a_heading_and_prose_is_not() {
        assert!(is_title("Lesson 1.2: MOVING BETWEEN PANES"));
        assert!(is_title("=   Welcome to Shannon Tutor - Version 0.4   ="));
        assert!(!is_title("Lessons learned the hard way"));
        assert!(!is_title("================"));
        assert!(is_rule("================"));
    }

    /// **Every `:help` the lessons send you to opens** — a topic the manual
    /// does not have would refuse the reader the lesson pointed them at.
    #[test]
    fn every_help_topic_it_names_exists() {
        let text = text();
        // A placeholder (`:help <topic>`) names no topic.
        let topics: Vec<&str> = (text.split("`:help ").skip(1))
            .filter_map(|rest| rest.split('`').next())
            .filter(|topic| !topic.starts_with('<'))
            .collect();
        assert!(topics.len() >= 5, "premise: it names topics: {topics:?}");
        for topic in topics {
            assert!(
                crate::help_view::anchor(topic).is_some(),
                "`:help {topic}` names nothing"
            );
        }
        // And every `|link|` it holds — its "Help files:" lists — read as
        // the page reads its marks.
        for line in text.split('\n') {
            for (at, mark) in crate::help::marks(line, crate::help::Dialect::Tutor) {
                let name = &line[at.start + 1..at.end - 1];
                if mark == crate::help::Mark::Link {
                    assert!(
                        crate::help_view::anchor(name).is_some(),
                        "|{name}| names nothing"
                    );
                }
            }
        }
    }

    /// The text ships whole: the version filled, and the title the splash
    /// and smoke look for.
    #[test]
    fn the_text_names_its_version() {
        let text = text();
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(!text.contains("{VERSION}"));
        assert!(text.contains("Welcome to Shannon Tutor"));
    }

    /// **What the tutor launches with keeps nothing of yours and writes
    /// nothing of yours** (review stop 16, C 1 and C 2): its directory made;
    /// no file of yours in its stack; the stub and a short prompt; a vault
    /// that keeps no practice secret past the run; a trust list kept in
    /// memory, which trusts for the run. Each was an arm of `main` that no
    /// test reached, and two wrote your real vault and trust file.
    #[test]
    fn a_launch_keeps_nothing_of_yours_and_writes_nothing_of_yours() {
        let parent = tempfile::tempdir().unwrap();
        let launch = Launch::prepare(parent.path()).unwrap();
        assert!(launch.dir.starts_with(parent.path()) && launch.dir.is_dir());
        assert_eq!(launch.stack.layers().count(), 0, "no file of yours");
        assert_eq!(
            (
                launch.config.tui.model.as_str(),
                launch.config.tui.prompt.height
            ),
            ("stub", 3)
        );
        let kept = shannon_core::Kept {
            name: "secret".into(),
            value: shannon_core::Secret::new("hunter2"),
            spendable: true,
        };
        let vault = &launch.vault;
        assert!(
            vault.keep("{{secret.0000}}", &kept).is_err(),
            "nothing kept"
        );
        assert_eq!(vault.handles().unwrap(), Vec::<String>::new());
        let mut trusted = launch.trusted.clone();
        let lasting = crate::trust::Lasting::Remembered;
        assert!(
            trusted.trust(&launch.dir, lasting).is_ok(),
            "trusted for the run"
        );
        assert!(trusted.covers(&launch.dir));
        assert_eq!(
            crate::trust::Trusted::load(None).0.dirs(),
            &[] as &[PathBuf],
            "and kept nowhere a second load could read"
        );
    }

    /// Made fresh, owner-only, seeded.
    #[test]
    fn its_directory_is_new_and_its_own() {
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().unwrap();
        let dir = prepare_dir(parent.path()).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let notes = dir.join("notes.txt");
        assert!(std::fs::read_to_string(&notes).unwrap().contains("tutor"));
        assert_eq!(
            std::fs::metadata(&notes).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    /// **Never one that already exists** — a directory someone put where
    /// the tutor's goes is refused, not seeded; a file someone put in it is
    /// refused, not truncated (review stop 16, B 2: `recursive(true)` and
    /// `truncate(true)` each passed every test).
    #[test]
    fn it_makes_nothing_over_what_is_there() {
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("there");
        std::fs::create_dir(&dir).unwrap();
        let e = make(&dir).expect_err("an existing directory refused");
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(!dir.join("notes.txt").exists(), "nothing seeded");
        std::fs::write(dir.join("notes.txt"), "planted").unwrap();
        let e = seed(&dir).expect_err("an existing file refused");
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read_to_string(dir.join("notes.txt")).unwrap(),
            "planted"
        );
    }
}
