//! **`:memories` and `:skills`** — what the model is told beyond the
//! conversation, as one screen of rows (Steven, 10-06: "an interface
//! similar to `:models`"; they were text pages, with vim's `~` past their
//! end, which said "a document" where they are lists). Three lists —
//! memories, skills, Claude Code's MCP servers — `Tab` forward and
//! `BackTab` back; `<Enter>` edits the file under the cursor in `$EDITOR`,
//! `x` disables it, `d` removes it, `n` and `N` make one.
//!
//! Pure: the app fills it from `knowledge::find` (`App::shelf_lists`) and
//! acts on its [`Effect`]; drawn and routed as a screen of `crate::setup`,
//! as `crate::model_picker` is. A row is a file (a server, on the MCP
//! list), never a line of text. What a key came to is said on the screen
//! ([`Screen::say`], [`Screen::tell`]) and, by the app, in `:messages`.

use std::path::PathBuf;

use crate::keymap::CursorMotion;
use crate::wrap::{clip, clip_middle, str_cells};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::setup::Effect;
use crate::style::Ink;
use crate::theme::Theme;

/// Which list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shelf {
    Memories,
    Skills,
    /// The MCP servers Claude Code runs: no file of Shannon's, so a row is
    /// disabled or enabled (`x`), never edited, removed or made here.
    Servers,
}

impl Shelf {
    const ALL: [Shelf; 3] = [Shelf::Memories, Shelf::Skills, Shelf::Servers];

    fn index(self) -> usize {
        match self {
            Shelf::Memories => 0,
            Shelf::Skills => 1,
            Shelf::Servers => 2,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Shelf::Memories => "Memories",
            Shelf::Skills => "Skills",
            Shelf::Servers => "MCP",
        }
    }
}

/// **What becomes of a file** — its badge, a word and a colour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    /// Sent whole with every turn: an instruction file, a memory named
    /// outright.
    Sent,
    /// A memory location's `MEMORY.md`: sent, and the model reads a memory
    /// when its line there is relevant.
    Index,
    /// Named to the model — by an index line, or the skills list — and read
    /// when wanted.
    Listed,
    /// Claude Code's own, in its session: `claude` reads it.
    Claude,
    /// Not read, and why.
    NotRead(String),
    /// Disabled by you: not told, not listed, a read of it refused; `x`
    /// enables it again.
    Disabled,
    /// An MCP server Claude Code has connected to.
    Connected,
    /// An MCP server that is not connected, and how it stands (`failed`,
    /// `needs-auth`, `pending`).
    Down(String),
}

impl Mark {
    fn word(&self) -> &'static str {
        match self {
            Mark::Sent => "sent",
            Mark::Index => "index",
            Mark::Listed => "listed",
            Mark::Claude => "claude's",
            Mark::NotRead(_) => "not read",
            Mark::Disabled => "disabled",
            Mark::Connected => "connected",
            Mark::Down(_) => "down",
        }
    }

    /// The badge's role — the theme says its colour.
    fn ink(&self) -> Ink {
        match self {
            Mark::Sent | Mark::Index => Ink::Safe,
            Mark::Listed => Ink::Text,
            Mark::Claude => Ink::Tool,
            Mark::NotRead(_) | Mark::Down(_) => Ink::Caution,
            Mark::Disabled => Ink::TextFaint,
            Mark::Connected => Ink::Safe,
        }
    }
}

/// **`x` on a row: disable its file, or enable it again** — what the app
/// turns into a `:set` of `[skills] disabled` (a skill by its name),
/// `[memory] disabled` (a file by its path) or `[mcp] disabled` (a server
/// by its name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toggle {
    pub shelf: Shelf,
    pub name: String,
    pub path: PathBuf,
    /// Disable it (`true`), or enable it again.
    pub disable: bool,
}

/// What a row says of its file: words, cut at the end where they do not
/// fit; or a place, cut in the middle, where its two ends are what tell
/// one from another (`~/…/my-project-3f9a2c71/memory`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum About {
    Text(String),
    Path(String),
}

/// One file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// A skill's name, a file's name.
    pub name: String,
    /// What it is for — a description, or where it is. A file not read says
    /// why instead (its [`Mark`]).
    pub about: About,
    pub path: PathBuf,
    pub mark: Mark,
}

/// Files under one heading: a `place` (a path, cut in the middle to fit)
/// or what they are, and a `note` that is never cut — "the model writes
/// here", which a cut place lost at the end of a long path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub place: String,
    pub note: String,
    pub rows: Vec<Row>,
}

/// A line under a list — what a write does, a location that named nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    /// Said as a caution: a write that asks where it should not.
    pub caution: bool,
}

/// One list: its groups, what it says when it has no file, its notes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct List {
    pub groups: Vec<Group>,
    pub empty: String,
    pub notes: Vec<Note>,
}

impl List {
    fn rows(&self) -> impl Iterator<Item = &Row> {
        self.groups.iter().flat_map(|g| &g.rows)
    }

    fn len(&self) -> usize {
        self.groups.iter().map(|g| g.rows.len()).sum()
    }
}

/// The widths of a row's columns: the cursor, the name, the badge.
const CURSOR: usize = 2;
const NAME: usize = 26;
const BADGE: usize = 10;
/// A page's step.
const PAGE: usize = 10;

/// `:memories` / `:skills`, open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    shelf: Shelf,
    /// The session it lists for, as its first line says it.
    header: String,
    lists: [List; 3],
    /// The cursor in each list, a row's index.
    at: [usize; 3],
    /// What the last key came to, until the next — why it did nothing
    /// (`true`, in the caution colour), or what it did.
    said: Option<(String, bool)>,
    /// A removal asked about, and what it removes: `y` removes, any other
    /// key keeps — asked HERE, since a question behind a full screen is one
    /// nobody sees.
    asking: Option<(String, Removal)>,
    /// `n`'s name, being typed: Enter makes it, Esc does not.
    naming: Option<String>,
    /// What the float shows beside the list: the selected file's text, as
    /// the app last read it ([`set_preview`](Self::set_preview)).
    preview: Option<Preview>,
}

/// **The file under the cursor, shown** — beside the list when the float is
/// wide enough (Steven, 10-07, fzf's `:GFiles`: a list, and the file it
/// points at). Its `text`, read by the app (`knowledge::peek`), and whether
/// that was cut; or why there is no text to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub path: PathBuf,
    pub text: Result<(String, bool), String>,
}

/// **`n`, named: a new memory or skill** — from a template, in `$EDITOR`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Create {
    pub shelf: Shelf,
    pub name: String,
}

/// **`d` on a row: remove its file** — asked first (`confirmed: false`),
/// then done (`true`); the app says what goes, and works it out again when
/// it is done, from the file as it is then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub shelf: Shelf,
    pub path: PathBuf,
    pub confirmed: bool,
    /// What the question said would go (the app's plan when it asked): a
    /// different plan at `y` is asked again, never done (review 29,
    /// Reviewers A and B: a directory that grew went with the old count).
    pub asked: Option<shannon_core::knowledge::Removal>,
}

impl Screen {
    pub fn new(shelf: Shelf, header: String, memories: List, skills: List) -> Screen {
        Screen {
            shelf,
            header,
            lists: [memories, skills, List::default()],
            at: [0, 0, 0],
            said: None,
            asking: None,
            naming: None,
            preview: None,
        }
    }

    /// **What the preview shows**, read by the app for the row under the
    /// cursor — none on an empty list.
    pub fn set_preview(&mut self, preview: Option<Preview>) {
        self.preview = preview;
    }

    /// The file the preview shows, for the app to tell whether the cursor
    /// has moved off it.
    pub fn preview_path(&self) -> Option<&std::path::Path> {
        self.preview.as_ref().map(|p| p.path.as_path())
    }

    /// **The preview's lines**, `width` cells wide, at most `rows` of them:
    /// the file's name, then its text, each line cut at the edge (a preview
    /// is a glance — `<Enter>` opens the file); what does not fit, and a
    /// file read only in part, said in the last row.
    pub fn preview_lines(&self, width: usize, rows: usize, theme: &Theme) -> Vec<Line<'static>> {
        let dim = theme.ui(Ink::TextQuiet);
        let Some(preview) = &self.preview else {
            return Vec::new();
        };
        let name = preview
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut out = vec![Line::from(Span::styled(
            clip(&name, width),
            Style::default().add_modifier(Modifier::BOLD),
        ))];
        let (text, cut) = match &preview.text {
            Ok((text, cut)) => (text, *cut),
            Err(why) => {
                for line in wrap(why, width) {
                    out.push(Line::from(Span::styled(line, dim)));
                }
                return out;
            }
        };
        let room = rows.saturating_sub(1);
        let lines: Vec<&str> = text.lines().collect();
        let more = lines.len() > room || cut;
        let shown = match more {
            true => room.saturating_sub(1),
            false => room,
        };
        for line in lines.iter().take(shown) {
            out.push(Line::from(clip(&line.replace('\t', "    "), width)));
        }
        if more && room > 0 {
            let said = match cut {
                true => "… the file goes on — Enter opens it",
                false => "… more below — Enter opens it",
            };
            out.push(Line::from(Span::styled(clip(said, width), dim)));
        }
        out
    }

    /// **The MCP servers' list**, set or read again — its cursor kept on its
    /// row where it still is.
    pub fn set_servers(&mut self, servers: List) {
        self.put(Shelf::Servers, servers);
    }

    /// **Ask about a removal** — `question` says what goes; `y` answers.
    pub fn ask(&mut self, question: String, removal: Removal) {
        self.asking = Some((question, removal));
    }

    /// Say why a key did nothing, on the screen, until the next key.
    pub fn say(&mut self, why: impl Into<String>) {
        self.said = Some((why.into(), true));
    }

    /// Say what a key did, on the screen, until the next key — not in the
    /// colour a refusal is said in.
    pub fn tell(&mut self, what: impl Into<String>) {
        self.said = Some((what.into(), false));
    }

    /// **The lists read again** — after an edit, a turn, a config change —
    /// each cursor kept on its row where it still exists.
    pub fn refill(&mut self, header: String, memories: List, skills: List) {
        self.header = header;
        self.put(Shelf::Memories, memories);
        self.put(Shelf::Skills, skills);
    }

    /// One list in, its cursor on the row it was on, where that still is;
    /// else at the same place, clamped.
    fn put(&mut self, shelf: Shelf, list: List) {
        let i = shelf.index();
        let kept = self.lists[i].rows().nth(self.at[i]).map(|r| r.path.clone());
        self.lists[i] = list;
        let len = self.lists[i].len();
        self.at[i] = kept
            .and_then(|p| self.lists[i].rows().position(|r| r.path == p))
            .unwrap_or(self.at[i])
            .min(len.saturating_sub(1));
    }

    /// The list showing.
    pub fn shelf(&self) -> Shelf {
        self.shelf
    }

    fn list(&self) -> &List {
        &self.lists[self.shelf.index()]
    }

    /// The file under the cursor, if the list has one.
    pub fn selected(&self) -> Option<&Row> {
        self.list().rows().nth(self.at[self.shelf.index()])
    }

    /// One key — plain or shifted only: `key.code` alone made `<C-q>` a `q`.
    pub fn key(&mut self, key: KeyEvent) -> Effect {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return Effect::None;
        }
        self.said = None;
        // A question asked takes the next key — `y` removes, any other key,
        // `Esc` included, keeps: the reflexive key is the safe one.
        if let Some((_, removal)) = self.asking.take() {
            if key.code == KeyCode::Char('y') {
                return Effect::Remove(Removal {
                    confirmed: true,
                    ..removal
                });
            }
            self.tell("kept — nothing removed");
            return Effect::None;
        }
        // A name being typed takes the keys a line editor would — and says
        // so of any other: `Tab` there did nothing, unsaid (review 29).
        if let Some(name) = self.naming.as_mut() {
            match key.code {
                KeyCode::Char(c) => name.push(c),
                KeyCode::Backspace => {
                    name.pop();
                }
                KeyCode::Enter if name.trim().is_empty() => {
                    self.say("a name first — Esc makes nothing");
                }
                KeyCode::Enter => {
                    let name = self.naming.take().unwrap_or_default();
                    return Effect::Create(Create {
                        shelf: self.shelf,
                        name,
                    });
                }
                KeyCode::Esc => {
                    self.naming = None;
                    self.tell("nothing made");
                }
                _ => self.say("typing a name — Enter makes it, Esc makes nothing"),
            }
            return Effect::None;
        }
        // A server is Claude Code's: no file of Shannon's to edit, remove or
        // make, so those keys say so — `x` is this list's one action.
        if self.shelf == Shelf::Servers
            && matches!(key.code, KeyCode::Enter | KeyCode::Char('d' | 'n' | 'N'))
        {
            self.say(
                "an MCP server is Claude Code's — `claude mcp` adds, removes and configures \
                 one; x disables it here",
            );
            return Effect::None;
        }
        let i = self.shelf.index();
        let last = self.list().len().saturating_sub(1);
        self.at[i] = match key.code {
            // `q` leaves; `Esc` does not — it is the key a vim hand presses
            // to get back to Normal, and here it closed the screen (Steven,
            // 10-07: "accidental kills"). As in vim's Normal mode, it does
            // nothing on the list.
            KeyCode::Char('q') => return Effect::Leave,
            KeyCode::Enter => {
                return match self.selected() {
                    Some(row) => Effect::Edit(row.path.clone()),
                    None => {
                        self.say(format!("nothing to edit — {}", self.list().empty));
                        Effect::None
                    }
                };
            }
            KeyCode::Char('x') => {
                let shelf = self.shelf;
                return match self.selected() {
                    Some(Row {
                        mark: Mark::NotRead(why),
                        ..
                    }) => {
                        let why = format!("not read already — {why}");
                        self.say(why);
                        Effect::None
                    }
                    Some(row) => Effect::Toggle(Toggle {
                        shelf,
                        name: row.name.clone(),
                        path: row.path.clone(),
                        disable: row.mark != Mark::Disabled,
                    }),
                    None => {
                        self.say("nothing to disable");
                        Effect::None
                    }
                };
            }
            KeyCode::Char('n') => {
                self.naming = Some(String::new());
                return Effect::None;
            }
            KeyCode::Char('N') => return Effect::AskModel(self.shelf),
            KeyCode::Char('d') => {
                let shelf = self.shelf;
                return match self.selected() {
                    Some(row) => Effect::Remove(Removal {
                        shelf,
                        path: row.path.clone(),
                        confirmed: false,
                        asked: None,
                    }),
                    None => {
                        self.say("nothing to remove");
                        Effect::None
                    }
                };
            }
            // Forward and back, as `gt`/`gT` are: every one of these turned
            // forward, so `BackTab` from the skills went on to MCP (review
            // 29, Reviewer C: D4).
            KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => {
                self.shelf = Shelf::ALL[(i + 1) % Shelf::ALL.len()];
                return Effect::None;
            }
            KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Left => {
                self.shelf = Shelf::ALL[(i + Shelf::ALL.len() - 1) % Shelf::ALL.len()];
                return Effect::None;
            }
            KeyCode::Home => 0,
            KeyCode::End => last,
            // **The rest is the shared keymap's** — `j` `k` with a count,
            // `gg` `G` `{n}G`, `<C-d>` `<C-f>`, through the app
            // ([`move_rows`](Self::move_rows), [`page_rows`](Self::page_rows)).
            // They were written out here, so `5j` was five `j`s' worth of
            // nothing and `gg` was `g` (Steven, 10-07: "share primitives").
            _ => return Effect::Navigate(key),
        };
        Effect::None
    }

    /// **A motion from the shared keymap, on the list's rows**: `j` `k`
    /// counted, `gg` `G` `{n}G`. `Err`, saying why, for one a list of rows
    /// has no use for — a column, a line of the window.
    pub fn move_rows(&mut self, motion: CursorMotion, count: usize) -> Result<(), &'static str> {
        let i = self.shelf.index();
        let (at, last) = (self.at[i], self.list().len().saturating_sub(1));
        self.at[i] = match motion {
            CursorMotion::Down => at.saturating_add(count.max(1)).min(last),
            CursorMotion::Up => at.saturating_sub(count.max(1)),
            CursorMotion::BufferStart => 0,
            CursorMotion::BufferEnd => last,
            CursorMotion::GotoLine(n) => n.saturating_sub(1).min(last),
            CursorMotion::Screen(_) => {
                return Err("the list keeps its cursor in view — gg and G go to its ends");
            }
            CursorMotion::Section { .. } => {
                return Err("a list has no sections — j and k move, Tab turns the list");
            }
            CursorMotion::Mark(..) => {
                return Err("a list has no marks — j and k move, Tab turns the list");
            }
            _ => return Err("a row has no columns — j and k move, Tab turns the list"),
        };
        Ok(())
    }

    /// **`<C-d>` `<C-f>` and back, on the list** — `rows` rows down (or up).
    pub fn page_rows(&mut self, down: bool, rows: usize) {
        let i = self.shelf.index();
        let (at, last) = (self.at[i], self.list().len().saturating_sub(1));
        self.at[i] = match down {
            true => at.saturating_add(rows).min(last),
            false => at.saturating_sub(rows),
        };
    }

    /// The rows a page of the list moves.
    pub const PAGE: usize = PAGE;

    /// **The page** into `out`, `width` cells wide and at most `rows` lines
    /// — the list scrolled to keep the cursor in view, what is above and
    /// below it said — and its legend. `crate::setup` boxes it.
    pub fn page(
        &self,
        out: &mut Vec<Line<'static>>,
        width: usize,
        rows: usize,
        theme: &Theme,
    ) -> &'static str {
        let dim = theme.ui(Ink::TextQuiet);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let chosen = theme.ui(Ink::Chosen);
        out.push(Line::from(Span::styled(clip(&self.header, width), bold)));
        let mut tabs = Vec::new();
        for shelf in Shelf::ALL {
            let count = self.lists[shelf.index()].len();
            let word = format!("{} {count}", shelf.title());
            tabs.push(match shelf == self.shelf {
                true => Span::styled(format!("[{word}]"), chosen),
                false => Span::styled(format!(" {word} "), dim),
            });
            tabs.push(Span::raw("  "));
        }
        tabs.push(Span::styled("Tab ⇄", dim));
        out.push(Line::from(tabs));

        // What the last key came to, a question, a name being typed: ABOVE
        // the list, where no length of it or of the notes cuts them (review
        // 29, Reviewer B: the question was the line a full box lost).
        let caution = theme.ui(Ink::Caution);
        if let Some((said, refused)) = &self.said {
            let style = if *refused { caution } else { Style::default() };
            for line in wrap(said, width) {
                out.push(Line::from(Span::styled(line, style)));
            }
        }
        if let Some((question, _)) = &self.asking {
            for line in wrap(question, width) {
                out.push(Line::from(Span::styled(
                    line,
                    caution.add_modifier(Modifier::BOLD),
                )));
            }
        }
        if let Some(name) = &self.naming {
            let what = match self.shelf {
                Shelf::Memories => "a new memory",
                Shelf::Skills => "a new skill",
                // Never named: `n` is refused on this list.
                Shelf::Servers => "a new server",
            };
            for line in wrap(&format!("{what}, its name: {name}▏"), width) {
                out.push(Line::from(Span::styled(line, chosen)));
            }
        }
        out.push(Line::from(""));

        // Sentences, wrapped — never cut: a note's path is what it says.
        let mut foot: Vec<Line<'static>> = Vec::new();
        let list = self.list();
        if !list.notes.is_empty() {
            foot.push(Line::from(""));
        }
        for note in &list.notes {
            let style = if note.caution { caution } else { dim };
            for line in wrap(&note.text, width) {
                foot.push(Line::from(Span::styled(line, style)));
            }
        }

        let (body, cursor) = self.body(width, theme);
        let room = rows.saturating_sub(out.len() + foot.len()).max(3);
        out.extend(window(body, cursor, room, width, theme));
        out.extend(foot);
        match (&self.asking, &self.naming) {
            // One item: `any` would read as a key of its own.
            (Some(_), _) => "y removes it, any other key keeps it",
            (None, Some(_)) => "Enter makes it in $EDITOR · Esc makes nothing",
            (None, None) => "Enter edit · x disable · d remove · n new · N ask the model · q close",
        }
    }

    /// The list's lines, and the line the cursor is on.
    fn body(&self, width: usize, theme: &Theme) -> (Vec<Line<'static>>, usize) {
        let dim = theme.ui(Ink::TextQuiet);
        let list = self.list();
        let at = self.at[self.shelf.index()];
        let mut lines = Vec::new();
        let mut cursor = 0;
        if list.len() == 0 {
            // A sentence, wrapped: where a file would go is what it says.
            for line in wrap(&list.empty, width.saturating_sub(2)) {
                lines.push(Line::from(Span::styled(format!("  {line}"), dim)));
            }
            return (lines, 0);
        }
        let mut index = 0;
        for group in &list.groups {
            if group.rows.is_empty() {
                continue;
            }
            // `  ── <place> · <note> ─…`: the note whole, the place in what
            // is left, at least three cells of rule after.
            let note = match group.note.is_empty() {
                true => String::new(),
                false => format!(" · {}", group.note),
            };
            let room = width.saturating_sub(5 + str_cells(&note) + 4);
            let title = format!("  ── {}{note} ", clip_middle(&group.place, room));
            let title = clip(&title, width.saturating_sub(1));
            let fill = "─".repeat(width.saturating_sub(str_cells(&title) + 1));
            lines.push(Line::from(Span::styled(format!("{title}{fill}"), dim)));
            for row in &group.rows {
                let here = index == at;
                if here {
                    cursor = lines.len();
                }
                lines.push(row_line(row, here, width, theme));
                index += 1;
            }
        }
        (lines, cursor)
    }
}

/// One file's row: the cursor, its name, its badge in its colour, what it
/// is for, dim.
fn row_line(row: &Row, here: bool, width: usize, theme: &Theme) -> Line<'static> {
    let name_style = match here {
        true => theme.ui(Ink::Chosen),
        false => Style::default(),
    };
    let room = width.saturating_sub(CURSOR + NAME + BADGE);
    let about = match (&row.mark, &row.about) {
        (Mark::NotRead(why), _) => clip(why, room),
        (_, About::Text(text)) => clip(text, room),
        (_, About::Path(path)) => clip_middle(path, room),
    };
    Line::from(vec![
        Span::styled(if here { "▸ " } else { "  " }, name_style),
        Span::styled(pad(&clip(&row.name, NAME - 1), NAME), name_style),
        Span::styled(pad(row.mark.word(), BADGE), theme.ui(row.mark.ink())),
        Span::styled(about, theme.ui(Ink::TextQuiet)),
    ])
}

/// `text` wrapped at spaces to `width` cells, a word wider than that broken —
/// the transcript's wrap (`wrap::wrap_cells_word`), not a third copy of it
/// beside `setup::fold` (Reviewer C, round 28; the copy's one-char floor was
/// all that kept a wide first character at width 1 from looping forever,
/// Reviewer B). No text, no lines.
fn wrap(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().collect();
    crate::wrap::wrap_cells_word(&chars, width.max(1))
        .into_iter()
        .map(|row| row.text)
        .collect()
}

/// **At most `room` of `lines`, the cursor's among them** — the first and
/// last of the window saying how many more lie beyond it, when any do.
fn window(
    lines: Vec<Line<'static>>,
    cursor: usize,
    room: usize,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    if lines.len() <= room {
        return lines;
    }
    let dim = theme.ui(Ink::TextQuiet);
    // Two lines kept for the "more" lines, and the cursor in the middle.
    let inner = room.saturating_sub(2).max(1);
    let top = cursor
        .saturating_sub(inner / 2)
        .min(lines.len().saturating_sub(inner));
    let end = (top + inner).min(lines.len());
    let more = |n: usize, way: &str| {
        Line::from(Span::styled(clip(&format!("  {way} {n} more"), width), dim))
    };
    let mut out = vec![match top {
        0 => Line::from(""),
        n => more(n, "↑"),
    }];
    out.extend(lines[top..end].iter().cloned());
    out.push(match lines.len() - end {
        0 => Line::from(""),
        n => more(n, "↓"),
    });
    out
}

/// `text` padded with spaces to `width` cells.
fn pad(text: &str, width: usize) -> String {
    format!(
        "{text}{}",
        " ".repeat(width.saturating_sub(str_cells(text)))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn row(name: &str, mark: Mark) -> Row {
        Row {
            name: name.into(),
            about: About::Text(format!("about {name}")),
            path: PathBuf::from(format!("/m/{name}")),
            mark,
        }
    }

    fn list(groups: &[(&str, &[&str])]) -> List {
        List {
            groups: groups
                .iter()
                .map(|(place, names)| Group {
                    place: place.to_string(),
                    note: String::new(),
                    rows: names.iter().map(|n| row(n, Mark::Listed)).collect(),
                })
                .collect(),
            empty: "none here".into(),
            notes: vec![Note {
                text: "WRITES — unasked".into(),
                caution: false,
            }],
        }
    }

    fn text(screen: &Screen, width: usize, rows: usize) -> Vec<String> {
        let mut out = Vec::new();
        screen.page(&mut out, width, rows, &Theme::terminal());
        out.iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// **A row is a file**: `j`/`k` move over files, never onto a heading,
    /// across groups; `<Enter>` edits the one under the cursor.
    #[test]
    fn enter_edits_the_file_under_the_cursor() {
        let mut s = Screen::new(
            Shelf::Memories,
            "h".into(),
            list(&[("a", &["one", "two"]), ("b", &["three"])]),
            List::default(),
        );
        assert_eq!(s.key(key(KeyCode::Enter)), Effect::Edit("/m/one".into()));
        s.move_rows(CursorMotion::Down, 1).unwrap();
        s.move_rows(CursorMotion::Down, 1).unwrap();
        assert_eq!(s.key(key(KeyCode::Enter)), Effect::Edit("/m/three".into()));
        s.move_rows(CursorMotion::Down, 1).unwrap();
        assert_eq!(
            s.selected().map(|r| r.name.as_str()),
            Some("three"),
            "the last stays"
        );
        s.move_rows(CursorMotion::BufferStart, 1).unwrap();
        assert_eq!(s.selected().map(|r| r.name.as_str()), Some("one"));
        // Navigation is the shared keymap's, through the app: the screen
        // hands the key back rather than moving on its own.
        let j = key(KeyCode::Char('j'));
        assert_eq!(s.key(j), Effect::Navigate(j));
        assert_eq!(s.selected().map(|r| r.name.as_str()), Some("one"));
        assert!(
            s.move_rows(CursorMotion::WordForward, 1).is_err(),
            "no columns"
        );
        assert_eq!(
            s.key(key(KeyCode::Esc)),
            Effect::Navigate(key(KeyCode::Esc)),
            "Esc is vim's way back to Normal — the keymap's — never a leave (Steven, \
             10-07)"
        );
        assert_eq!(s.key(key(KeyCode::Char('q'))), Effect::Leave);
        let ctrl = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(s.key(ctrl), Effect::None, "<C-q> is not q");
    }

    /// **An empty list says what to do**, and `<Enter>` there says why it
    /// opens nothing — a dead key reads as broken.
    #[test]
    fn an_empty_list_says_where_files_go() {
        let skills = List {
            empty: "a skill is <name>/SKILL.md under ~/.config/shannon/skills".into(),
            ..List::default()
        };
        let mut s = Screen::new(Shelf::Skills, "h".into(), List::default(), skills);
        assert_eq!(s.key(key(KeyCode::Enter)), Effect::None);
        let shown = text(&s, 74, 30).join("\n");
        assert!(shown.contains("nothing to edit — a skill is"), "{shown}");
        s.key(key(KeyCode::Char('j')));
        assert!(
            !text(&s, 74, 30).join("\n").contains("nothing to edit"),
            "said once"
        );
    }

    /// **`Tab` turns to the other list**, each keeping its own cursor; the
    /// header names both, with counts, the open one marked.
    #[test]
    fn tab_turns_the_list_and_each_keeps_its_cursor() {
        let mut s = Screen::new(
            Shelf::Memories,
            "Session x".into(),
            list(&[("a", &["one", "two"])]),
            list(&[("s", &["review"])]),
        );
        s.move_rows(CursorMotion::Down, 1).unwrap();
        s.key(key(KeyCode::Tab));
        assert_eq!(s.shelf(), Shelf::Skills);
        assert_eq!(s.selected().map(|r| r.name.as_str()), Some("review"));
        let shown = text(&s, 74, 30);
        assert!(
            shown[1].contains("[Skills 1]")
                && shown[1].contains("Memories 2")
                && shown[1].contains("MCP 0"),
            "{shown:?}"
        );
        s.key(key(KeyCode::Tab));
        assert_eq!(s.shelf(), Shelf::Servers, "then the MCP list");
        s.key(key(KeyCode::Tab));
        assert_eq!(
            s.selected().map(|r| r.name.as_str()),
            Some("two"),
            "and round"
        );
        // Back as well as forward (review 29, Reviewer C: D4 — every one
        // of these turned forward).
        for (back, forward) in [
            (KeyCode::BackTab, KeyCode::Tab),
            (KeyCode::Char('h'), KeyCode::Char('l')),
            (KeyCode::Left, KeyCode::Right),
        ] {
            s.key(key(back));
            assert_eq!(s.shelf(), Shelf::Servers, "{back:?} turns back, round");
            s.key(key(forward));
            assert_eq!(s.shelf(), Shelf::Memories, "{forward:?} turns forward");
        }
    }

    /// **What was said, a question, a name being typed stand ABOVE the
    /// list** — a box full of notes cut the question off the bottom
    /// (review 29, Reviewer B: M6); a refusal is said in the caution
    /// colour, an outcome not (drawn, not only marked); any key a name
    /// does not take is said.
    #[test]
    fn what_is_said_and_asked_stands_above_the_list() {
        let mut memories = list(&[("a", &["one", "two", "three"])]);
        memories.notes = (0..40)
            .map(|i| Note {
                text: format!("note {i}"),
                caution: false,
            })
            .collect();
        let mut s = Screen::new(Shelf::Memories, "h".into(), memories, List::default());
        let removal = Removal {
            shelf: Shelf::Memories,
            path: "/m/one".into(),
            confirmed: false,
            asked: None,
        };
        s.ask("remove `one`?".into(), removal);
        let shown = text(&s, 60, 12);
        assert!(
            shown[..4].iter().any(|l| l.contains("remove `one`?")),
            "{shown:?}"
        );

        s.key(key(KeyCode::Esc));
        s.key(key(KeyCode::Char('n')));
        s.key(key(KeyCode::Char('z')));
        s.key(key(KeyCode::Tab));
        let shown = text(&s, 60, 12);
        assert!(
            shown[..5].iter().any(|l| l.contains("its name: z▏")),
            "{shown:?}"
        );
        assert!(
            shown
                .iter()
                .any(|l| l.contains("typing a name — Enter makes it")),
            "Tab while naming is said: {shown:?}"
        );

        let theme = Theme::terminal();
        let style_of = |s: &Screen, said: &str| {
            let mut out = Vec::new();
            s.page(&mut out, 60, 30, &theme);
            out.iter()
                .flat_map(|l| &l.spans)
                .find(|span| span.content.contains(said))
                .map(|span| span.style.fg)
                .unwrap_or_else(|| panic!("{said} not drawn"))
        };
        let caution = Some(theme.fg(Ink::Caution));
        s.say("no such thing");
        assert_eq!(style_of(&s, "no such thing"), caution);
        s.tell("done it");
        assert_ne!(style_of(&s, "done it"), caution);
    }

    /// **The page fits its box**: every line within the width, no more lines
    /// than the rows, the cursor's row on it, and what is cut said.
    #[test]
    fn a_long_list_scrolls_to_the_cursor() {
        let names: Vec<String> = (0..60).map(|i| format!("memory_{i:02}.md")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut s = Screen::new(
            Shelf::Memories,
            "a header much longer than the box is wide, which must be cut and not folded".into(),
            list(&[(
                "~/.config/shannon/projects/a-very-long-project-name/memory",
                &refs,
            )]),
            List::default(),
        );
        for _ in 0..40 {
            s.move_rows(CursorMotion::Down, 1).unwrap();
        }
        let shown = text(&s, 60, 20);
        assert!(shown.len() <= 20, "{}", shown.len());
        for line in &shown {
            assert!(str_cells(line) <= 60, "{line:?}");
        }
        assert!(
            shown.iter().any(|l| l.starts_with("▸ memory_40.md")),
            "{shown:#?}"
        );
        assert!(shown.iter().any(|l| l.contains("↑") && l.contains("more")));
        assert!(shown.iter().any(|l| l.contains("↓") && l.contains("more")));
    }

    /// **Every mark, from the type** — `place` has no `_`, so a mark added is
    /// a compile error until it is placed here. Two tests hand-listed five of
    /// eight between them, `Connected` and `Down` in neither (review stop 12,
    /// T7).
    fn every_mark() -> Vec<Mark> {
        let all = vec![
            Mark::Sent,
            Mark::Index,
            Mark::Listed,
            Mark::Claude,
            Mark::NotRead("a credential path".into()),
            Mark::Disabled,
            Mark::Connected,
            Mark::Down("failed".into()),
        ];
        let place = |mark: &Mark| match mark {
            Mark::Sent => 0,
            Mark::Index => 1,
            Mark::Listed => 2,
            Mark::Claude => 3,
            Mark::NotRead(_) => 4,
            Mark::Disabled => 5,
            Mark::Connected => 6,
            Mark::Down(_) => 7,
        };
        for (i, mark) in all.iter().enumerate() {
            assert_eq!(place(mark), i, "{mark:?} in its place");
        }
        all
    }

    /// A mark's badge colour, as its row draws it.
    fn badge_fg(mark: &Mark) -> Option<ratatui::style::Color> {
        let line = row_line(&row("x", mark.clone()), false, 74, &Theme::terminal());
        (line.spans.iter())
            .find(|s| s.content.trim() == mark.word())
            .and_then(|s| s.style.fg)
    }

    /// **A badge's colour says what becomes of the file** — on the rendered
    /// span, not the enum: a mark painted like its neighbour tells nothing.
    /// Every file mark its own, but the index, which is sent as a memory is
    /// (`sent`'s, by design; Reviewer C, round 28); a disabled one unlike
    /// all; a server's two apart — sharing the files' green and yellow,
    /// which reads as their hue's sense (measured, pinned as it is).
    #[test]
    fn each_mark_is_its_own_colour_on_screen() {
        let all = every_mark();
        let colours: Vec<_> = all.iter().map(badge_fg).collect();
        let (index, connected, down) = (1, 6, 7);
        let files: Vec<usize> = (0..all.len())
            .filter(|&i| i != index && i < connected)
            .collect();
        for &i in &files {
            assert!(colours[i].is_some(), "{:?} is painted", all[i]);
            for &j in files.iter().filter(|&&j| j > i) {
                assert_ne!(
                    colours[i], colours[j],
                    "{:?} and {:?} read alike",
                    all[i], all[j]
                );
            }
        }
        assert_eq!(colours[index], colours[0], "the index is sent");
        assert_ne!(
            colours[connected], colours[down],
            "a server up and down, apart"
        );
        assert_eq!(
            colours[connected], colours[0],
            "measured: up is sent's green"
        );
        assert_eq!(
            colours[down], colours[4],
            "measured: down is not-read's yellow"
        );
        let theme = Theme::terminal();
        let refused = row_line(
            &row("x", Mark::NotRead("a credential path".into())),
            false,
            74,
            &theme,
        );
        let said: String = refused.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            said.contains("a credential path"),
            "why it is not read: {said}"
        );
    }

    /// **A long place is cut in the middle, and its note is never cut** —
    /// "the model writes here" fell off the end of a long path; a note's
    /// sentence wraps.
    #[test]
    fn a_place_is_cut_in_the_middle_and_its_note_kept() {
        let place = "~/.config/shannon/projects/my-project-3f9a2c71/memory";
        let mut memories = list(&[(place, &["MEMORY.md"])]);
        memories.groups[0].note = "the model writes here".into();
        memories.notes[0].text = format!("Writes: the model writes to {place} unasked");
        let s = Screen::new(Shelf::Memories, "h".into(), memories, List::default());
        let shown = text(&s, 60, 30);
        let heading = shown.iter().find(|l| l.contains("──")).expect("a heading");
        assert!(heading.contains("the model writes here"), "{heading}");
        assert!(
            heading.contains("~/.conf") && heading.contains('…') && heading.contains("/memory"),
            "both ends kept: {heading}"
        );
        let writes = shown.join("\n");
        assert!(
            writes.contains("3f9a2c71/memory") && writes.contains("unasked"),
            "wrapped whole: {writes}"
        );
        for line in &shown {
            assert!(str_cells(line) <= 60, "{line:?}");
        }
    }

    /// **The page draws at any width, however narrow** — an `About::Path`
    /// row cut in the middle with no room at all, and a note whose first
    /// character is wider than the line. Both were held by one line each
    /// that no test reached: without them `:memories` panicked at 40 columns
    /// and looped forever at width 1 (Reviewer B, round 28).
    #[test]
    fn the_page_draws_at_any_width() {
        let mut memories = list(&[("~/.claude/projects/-Users-me-proj/memory", &["MEMORY.md"])]);
        memories.groups[0].rows[0].about = About::Path("/Users/me/proj".into());
        memories.notes.push(Note {
            text: "日本語 の メモ".into(),
            caution: true,
        });
        let s = Screen::new(Shelf::Memories, "h".into(), memories, List::default());
        // A row's columns are fixed, and `setup` folds what overflows; what
        // is asked here is that a page comes back, its note in it.
        for width in [80, 40, 36, 5, 1] {
            let shown = text(&s, width, 30);
            assert!(shown.iter().any(|l| l.contains('日')), "{width}: {shown:?}");
        }
        let _ = text(&s, 0, 30);
    }

    /// **A refill keeps the cursor on its file**, where it still is.
    #[test]
    fn a_refill_keeps_the_cursor_on_its_file() {
        let mut s = Screen::new(
            Shelf::Memories,
            "h".into(),
            list(&[("a", &["one", "two", "three"])]),
            List::default(),
        );
        s.move_rows(CursorMotion::Down, 1).unwrap();
        s.move_rows(CursorMotion::Down, 1).unwrap();
        s.refill(
            "h".into(),
            list(&[("a", &["new", "one", "two", "three"])]),
            List::default(),
        );
        assert_eq!(s.selected().map(|r| r.name.as_str()), Some("three"));
        // Gone from mid-list: the cursor keeps its place, on the file that
        // took it — not back at the top (Reviewer C, round 28: the end-of-list
        // case below lands on the top either way).
        s.move_rows(CursorMotion::Up, 1).unwrap();
        assert_eq!(s.selected().map(|r| r.name.as_str()), Some("two"));
        s.refill(
            "h".into(),
            list(&[("a", &["new", "one", "three"])]),
            List::default(),
        );
        assert_eq!(s.selected().map(|r| r.name.as_str()), Some("three"));
        s.refill("h".into(), list(&[("a", &["one"])]), List::default());
        assert_eq!(
            s.selected().map(|r| r.name.as_str()),
            Some("one"),
            "gone: clamped"
        );
    }

    /// **`x` disables the file under the cursor, or enables it again** — a
    /// row not read already says why instead, and an empty list says there
    /// is nothing.
    #[test]
    fn x_toggles_the_file_under_the_cursor() {
        let mut memories = list(&[("a", &["one", "two"])]);
        memories.groups[0].rows[1].mark = Mark::Disabled;
        memories.groups[0]
            .rows
            .push(row("key", Mark::NotRead("a credential path".into())));
        let mut s = Screen::new(Shelf::Memories, "h".into(), memories, List::default());
        let toggled = |effect: Effect| match effect {
            Effect::Toggle(t) => Some((t.name, t.disable)),
            _ => None,
        };
        assert_eq!(
            toggled(s.key(key(KeyCode::Char('x')))),
            Some(("one".into(), true))
        );
        s.move_rows(CursorMotion::Down, 1).unwrap();
        assert_eq!(
            toggled(s.key(key(KeyCode::Char('x')))),
            Some(("two".into(), false))
        );
        s.move_rows(CursorMotion::Down, 1).unwrap();
        assert_eq!(s.key(key(KeyCode::Char('x'))), Effect::None);
        assert!(
            text(&s, 74, 30)
                .join("\n")
                .contains("not read already — a credential path"),
            "said why"
        );
        s.key(key(KeyCode::Tab));
        assert_eq!(
            s.key(key(KeyCode::Char('x'))),
            Effect::None,
            "no skill there"
        );
    }
}
