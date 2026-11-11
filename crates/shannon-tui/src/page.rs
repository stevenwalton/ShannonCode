//! **A viewer's page, as a buffer** (`docs/DesignDocs/buffers.md` §10, V3) —
//! the manual `:help` shows, `:analyze`'s report, the `:messages` history,
//! and every listing (`:ls`, `:alias`, `:rules`, `:scriptnames`, `:themes`;
//! V5.1), one buffer each. Unlisted, as vim's help buffer is: `:ls!` names
//! it, `:b N` shows it, and closing the last window on it keeps it — with
//! where you were in it, which `:b N` returns to (vim's `'"` mark).
//!
//! **A page carries its recipe** ([`Recipe`], Emacs's
//! `revert-buffer-function`): what it is and how its lines are made — the
//! manual composed once, a report each `:analyze` begins, the history read
//! again when it is opened, a listing derived again every frame a window
//! shows it. A listing's settings are the recipe's, so they are the
//! BUFFER's: `:ls!` is the one `:ls` buffer naming the unlisted too (Steven,
//! 10-25: "the same list, showing more"), as Emacs's `*Buffer List*` carries
//! its files-only flag (measured, `docs/Research/buffers-and-windows-prior-art.md`
//! §3b).
//!
//! **A window mirrors its page** (§10.3, revised before building): each
//! window holds a [`TextView`] — its own cursor, scroll and selection, over
//! its own copy of the page's text, as a transcript window holds a copy of
//! its session's (`SessionView::chat`) — and takes the page again when its
//! version moved ([`PageBuffer::sync`]). A `TextView`'s cursor is the
//! editor's own buffer and carries the text it moves through, so splitting
//! the page from the window would split nothing.

use crate::paint::PageLine;
use crate::prompt::{Prompt, Registers};
use crate::text_view::TextView;
use crate::theme_picker::ThemeList;
use crate::viewport::Place;
use crate::wm::BufferId;
use crate::workspace::{PageKind, PaneKind};

/// **What a page is, and how its lines are made** — kept on the buffer, so
/// a page knows how to make itself again (Emacs's `revert-buffer-function`),
/// and what it is is said once: [`PageBuffer::of`] takes everything else a
/// page starts with from it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Recipe {
    /// The manual, composed once (`help_view::manual`).
    Manual,
    /// `:analyze <command>`'s report — each command begins it again
    /// ([`PageBuffer::begin`]); its windows titled by the command.
    Report,
    /// The notice history — read again each time `:messages` opens it
    /// ([`PageBuffer::renew`]), a new window at its end, newest last.
    Messages,
    /// A live listing — derived again every frame a window shows it
    /// (`App::renew_listings`).
    Listing(Listing),
    /// `:themes` — live too, its page a function of the theme worn over the
    /// themes as their places were read: `None` until the next derive reads
    /// them, which `:themes` typed asks for again. Not a [`Listing`]: those
    /// are derived from what the app holds, this from a list it read once
    /// (its places are hundreds of files, so never every frame) and keeps.
    Themes(Option<ThemeList>),
    /// `shannon tutor`'s lessons (`crate::tutor`) — composed once, then
    /// yours to change: the one modifiable page ([`PageBuffer::edit`]),
    /// vimtutor's copy, edited in place and never written. With the
    /// pictures its lessons draw in (`tutor::Lessons`), so every repaint of
    /// an edit paints them as drawn.
    Tutor(crate::tutor::Lessons),
}

/// **A live listing's recipe**, its settings with it — the buffer's, so
/// every window on it shows one list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Listing {
    /// `:ls` — the sessions open this run; `all` is `:ls!`'s: the pages and
    /// the session trees too. `:ls!` sets it and `:ls` clears it, as
    /// Emacs's `C-u C-x C-b` and `C-x C-b` do (measured).
    Buffers { all: bool },
    /// `:alias` (`:secrets`) — the substitution table.
    Aliases,
    /// `:rules` — the permission policy in force.
    Rules,
    /// `:scriptnames` — the config files in effect.
    Scripts,
    /// `:tabs` — the tab pages and their windows.
    Tabs,
    /// `:marks` — every session's marks, yours then the agent's.
    Marks,
}

impl From<Listing> for Recipe {
    fn from(listing: Listing) -> Recipe {
        Recipe::Listing(listing)
    }
}

impl Recipe {
    /// What shows a page made by it.
    pub(crate) fn kind(&self) -> PaneKind {
        match self {
            Recipe::Manual => PaneKind::Page(PageKind::Help),
            Recipe::Report => PaneKind::Page(PageKind::Analyze),
            Recipe::Messages => PaneKind::Page(PageKind::Messages),
            Recipe::Listing(Listing::Buffers { .. }) => PaneKind::Page(PageKind::Buffers),
            Recipe::Listing(Listing::Aliases) => PaneKind::Page(PageKind::Aliases),
            Recipe::Listing(Listing::Rules) => PaneKind::Page(PageKind::Rules),
            Recipe::Listing(Listing::Scripts) => PaneKind::Page(PageKind::Scripts),
            Recipe::Listing(Listing::Tabs) => PaneKind::Page(PageKind::Tabs),
            Recipe::Listing(Listing::Marks) => PaneKind::Page(PageKind::Marks),
            Recipe::Themes(_) => PaneKind::Page(PageKind::Themes),
            Recipe::Tutor(_) => PaneKind::Page(PageKind::Tutor),
        }
    }
}

/// One page buffer: its id, its recipe, what it is called, its lines,
/// where you last were in it, and which version of it each window holds.
#[derive(Debug)]
pub(crate) struct PageBuffer {
    id: BufferId,
    recipe: Recipe,
    /// What `:ls!` and a refusal call it — "the manual", or `:analyze`'s
    /// command.
    name: String,
    /// **What every window's title names**, unless the window was opened at
    /// a topic of its own (`:help windows`) — `:analyze`'s command, a
    /// [`Recipe::Report`]'s.
    heading: Option<String>,
    lines: Vec<PageLine>,
    /// **What each line names** ([`Row`]), beside it — a listing's; empty
    /// for a page whose lines name nothing (the manual, a report, the
    /// history), so they carry no copy of a column of `None`s.
    rows: Vec<Option<Row>>,
    /// **Where you last were in it** — the source line of the cursor in the
    /// window on it you were last in, as of the last event (`App::settle`);
    /// vim's `'"`. Kept when its last window closes, so `:b N` opens there.
    left_at: Option<usize>,
    /// Bumped by every change to its lines: a window holding an older one
    /// takes them again ([`sync`](Self::sync)).
    version: u64,
    /// **The version it last became a different document at** — `:analyze`
    /// of another command ([`begin`](Self::begin)). A window from before it
    /// starts over; one from after it keeps its place through a newer
    /// version of the same document ([`renew`](Self::renew)).
    began: u64,
    /// **A new window starts at the end** — `:messages`, newest last, vim's
    /// order (a [`Recipe::Messages`]'); at the top otherwise.
    from_the_end: bool,
    /// **The one editor of a modifiable page** — its undo, its insert in
    /// flight — made at its first [`edit`](Self::edit); `None` until then,
    /// and always on a page that is read. Boxed: one page has it, and every
    /// buffer in the list would carry its size.
    editor: Option<Box<Prompt>>,
    /// **How its latest edits moved its lines** — each with the version it
    /// made, for a window behind to follow its text ([`sync`](Self::sync)).
    /// Empty on a page that is read.
    shifts: std::collections::VecDeque<(u64, Shift)>,
}

/// **What a line of a page names** — the id its row carries, as Emacs's
/// `tabulated-list-id` is a property of the line: what `<CR>` acts on
/// (`App::act_on_row`), and what a window keeps its place by when the page
/// is made again ([`PageBuffer::sync`]). A row may take several lines (a
/// process left running and its last line); a heading, a note or a blank
/// names nothing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Row {
    /// A buffer — a session, a page or a tree (`:ls`).
    Buffer(BufferId),
    /// A process a finished command left running, by the call that started
    /// it (`:ls`, below the buffers).
    Leftover(String),
    /// An alias or a secret, by its handle (`:alias`).
    Alias(String),
    /// A rule, as `:rules` lists it, under the heading it is listed under.
    Rule(String),
    /// A config file in effect, by its path (`:scriptnames`).
    Script(std::path::PathBuf),
    /// A theme, by its name (`:themes`).
    Theme(String),
    /// A tab page's heading (`:tabs`), by a pane in it — the tab is wherever
    /// that pane is, so the row keeps naming its tab when a tab before it
    /// closes. `<CR>` goes to it. It was the tab's place, from 0: the cursor
    /// stayed on that place and slid into another tab's block (review stop
    /// 11, F 9).
    Tab(crate::wm::PaneId),
    /// A window (`:tabs`), by its pane — `<CR>` stands you in it, its tab
    /// shown (F 10: a line per window invites a `<CR>` per window).
    Pane(crate::wm::PaneId),
    /// A mark (`:marks`), by its session and its name, yours or the
    /// agent's — `<CR>` goes to it.
    Mark {
        buffer: BufferId,
        whose: crate::keymap::Whose,
        name: char,
    },
}

/// **A page's lines, each with what its row names** — built a line at a
/// time, the line and its row pushed together, so the two cannot drift
/// apart: a listing's recipe makes one (`App::listing_lines`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Lines {
    lines: Vec<PageLine>,
    rows: Vec<Option<Row>>,
}

impl Lines {
    /// A line, naming `row` or nothing.
    pub(crate) fn push(&mut self, row: Option<Row>, line: impl Into<PageLine>) {
        self.lines.push(line.into());
        self.rows.push(row);
    }

    /// Each line of plain `text`, naming nothing — a heading, a note, a
    /// blank.
    pub(crate) fn text(&mut self, text: &str) {
        for line in text.lines() {
            self.push(None, plain(line));
        }
    }

    /// Each line of plain `text`, naming `row` — a row may take several.
    pub(crate) fn row(&mut self, row: &Row, text: &str) {
        for line in text.lines() {
            self.push(Some(row.clone()), plain(line));
        }
    }

    pub(crate) fn lines(&self) -> &[PageLine] {
        &self.lines
    }

    /// What line `at` names, if anything.
    #[cfg(test)]
    pub(crate) fn row_at(&self, at: usize) -> Option<&Row> {
        self.rows.get(at)?.as_ref()
    }

    /// The lines as plain text, one to a line — what `:ls` reads as when a
    /// test wants its words.
    #[cfg(test)]
    pub(crate) fn plain_text(&self) -> String {
        let lines: Vec<String> = self.lines.iter().map(PageLine::text).collect();
        lines.join("\n")
    }
}

/// A line of plain text, in the app's own colours.
fn plain(text: &str) -> PageLine {
    ratatui::text::Line::from(text.to_string()).into()
}

/// **Where each line of `old` went in `new`**, by what its row names: the
/// k-th line of a row to the k-th line of the same row; a line naming
/// nothing, or a row gone, nowhere — left to its index.
fn moves(old: &[Option<Row>], new: &[Option<Row>]) -> Vec<Option<usize>> {
    use std::collections::HashMap;
    let mut lines_of: HashMap<&Row, Vec<usize>> = HashMap::new();
    for (at, row) in new.iter().enumerate() {
        if let Some(row) = row {
            lines_of.entry(row).or_default().push(at);
        }
    }
    let mut seen: HashMap<&Row, usize> = HashMap::new();
    old.iter()
        .map(|row| {
            let row = row.as_ref()?;
            let nth = seen.entry(row).or_default();
            let to = lines_of.get(row).and_then(|lines| lines.get(*nth)).copied();
            *nth += 1;
            to
        })
        .collect()
}

impl PageBuffer {
    /// **The page buffer `id` that `recipe` makes** — each page's character
    /// said once, here: the manual composed; `:analyze`'s windows titled by
    /// its command; the notice history read from the end, newest last; a
    /// listing named for what it lists, its lines its first derive's.
    pub(crate) fn of(id: BufferId, recipe: Recipe) -> PageBuffer {
        let (name, lines) = match &recipe {
            Recipe::Manual => ("the manual", crate::help_view::manual()),
            Recipe::Report => ("", Vec::new()),
            Recipe::Messages => ("the notice history", Vec::new()),
            Recipe::Listing(Listing::Buffers { .. }) => ("the buffer list", Vec::new()),
            Recipe::Listing(Listing::Aliases) => ("the aliases", Vec::new()),
            Recipe::Listing(Listing::Rules) => ("the rules", Vec::new()),
            Recipe::Listing(Listing::Scripts) => ("the config files", Vec::new()),
            Recipe::Listing(Listing::Tabs) => ("the tab list", Vec::new()),
            Recipe::Listing(Listing::Marks) => ("the marks", Vec::new()),
            Recipe::Themes(_) => ("the themes", Vec::new()),
            Recipe::Tutor(lessons) => (
                "the tutor",
                crate::tutor::lines(
                    &crate::tutor::drawn_in(&lessons.text, &lessons.pictures),
                    &lessons.pictures,
                ),
            ),
        };
        PageBuffer {
            id,
            heading: matches!(recipe, Recipe::Report).then(String::new),
            from_the_end: matches!(recipe, Recipe::Messages),
            recipe,
            name: name.to_string(),
            lines,
            rows: Vec::new(),
            left_at: None,
            version: 0,
            began: 0,
            editor: None,
            shifts: std::collections::VecDeque::new(),
        }
    }

    /// **An edit of its text, made in `window`** — vim's `'modifiable'`, for
    /// the page that has it ([`PageKind::modifiable`]). `f` is handed the
    /// page's one editor with the window's buffer lent to it
    /// (`TextView::edit`): the text it leaves is the page's, which every
    /// other window on it takes at its next [`sync`](Self::sync), and so is
    /// the undo, as vim's is a buffer's — `u` in one window undoes an edit
    /// made in another; the cursor stays each window's. The editor is made at
    /// the first edit, sharing `registers`: a yank here is a `p` in a prompt.
    /// `None` for a page that is read, which nothing changes.
    pub(crate) fn edit<R>(
        &mut self,
        window: &mut PageWindow,
        registers: &Registers,
        scrolloff: usize,
        f: impl FnOnce(&mut Prompt) -> R,
    ) -> Option<R> {
        if !matches!(self.kind(), PaneKind::Page(kind) if kind.modifiable()) {
            return None;
        }
        // On the text the page holds: a window behind it takes it first.
        self.sync(window);
        let before = window.text.text();
        let editor = (self.editor).get_or_insert_with(|| Box::new(Prompt::sharing(registers)));
        let out = window.text.edit(editor, scrolloff, f);
        let after = window.text.text();
        if after != before {
            // Painted as `of` paints it: the tutor is the one page
            // `PageKind::modifiable` names; a second says its paint here.
            let none = crate::tutor::Pictures::default();
            let pictures = match &self.recipe {
                Recipe::Tutor(lessons) => &lessons.pictures,
                _ => &none,
            };
            self.lines = crate::tutor::lines(&after, pictures);
            self.record(Shift::between(&before, &after));
            window.text.restyle(self.lines.clone());
        }
        window.synced = self.version;
        Some(out)
    }

    /// **A new version of the page, and what it did to its lines** — so a
    /// window further behind follows its text ([`shifts_since`](Self::shifts_since)).
    fn record(&mut self, shift: Shift) {
        self.version += 1;
        if self.shifts.len() == MAX_SHIFTS {
            self.shifts.pop_front();
        }
        self.shifts.push_back((self.version, shift));
    }

    /// **How each line moved since `synced`**, edit by edit, oldest first —
    /// `None` when that is not all known (a version no edit made, or one
    /// older than the shifts kept), and the caller keeps lines by index.
    fn shifts_since(&self, synced: u64) -> Option<Vec<Shift>> {
        let since: Vec<&(u64, Shift)> = (self.shifts.iter())
            .filter(|(version, _)| *version > synced)
            .collect();
        let whole = since.len() as u64 == self.version - synced
            && since
                .first()
                .is_some_and(|(version, _)| *version == synced + 1);
        whole.then(|| since.iter().map(|(_, shift)| *shift).collect())
    }

    /// **Drop the insert in flight**, for a page whose window the cursor has
    /// left — what `App::end_what_focus_left` does to each prompt's draft
    /// (`Prompt::end_insert_session`), here for the page's one editor.
    pub(crate) fn end_insert_session(&mut self) {
        if let Some(editor) = self.editor.as_mut() {
            editor.end_insert_session();
        }
    }

    pub(crate) fn id(&self) -> BufferId {
        self.id
    }

    /// What shows it — `PaneKind::Page(PageKind::Help)`, `Analyze`, `Messages`, or a
    /// listing's kind.
    pub(crate) fn kind(&self) -> PaneKind {
        self.recipe.kind()
    }

    /// How it is made, its settings with it.
    pub(crate) fn recipe(&self) -> &Recipe {
        &self.recipe
    }

    /// How it is made, to change a setting (`:ls!`) or read a listing's
    /// places again (`:themes`) — the next derive makes it so.
    pub(crate) fn recipe_mut(&mut self) -> &mut Recipe {
        &mut self.recipe
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// **What a window's title calls it** — its kind's word, `ls!` while
    /// `:ls` names the unlisted too: a setting of the buffer, so a scope
    /// changed from another pane shows in every window on it (G4).
    pub(crate) fn label(&self) -> &'static str {
        match self.recipe {
            Recipe::Listing(Listing::Buffers { all: true }) => "ls!",
            _ => self.kind().title(),
        }
    }

    /// **How a message names it** — "the manual", or a page named by its
    /// command as that command, typeable: "`:analyze ls -la`". "b4 is ls -la,
    /// not a session" read as though `ls -la` were a session's name.
    pub(crate) fn described(&self) -> String {
        match &self.heading {
            Some(command) => format!("`:{} {command}`", self.kind().title()),
            None => self.name.clone(),
        }
    }

    /// Where you last were in it — vim's `'"`.
    pub(crate) fn left_at(&self) -> Option<usize> {
        self.left_at
    }

    /// Note where a window on it is: its cursor's source line.
    pub(crate) fn leave_at(&mut self, line: usize) {
        self.left_at = Some(line);
    }

    /// **It becomes a different document** — `:analyze` of another command,
    /// as vim's help buffer moves to another file: every window on it starts
    /// over at its start, and where you were in the old one is forgotten.
    pub(crate) fn begin(&mut self, name: impl Into<String>, lines: Vec<PageLine>) {
        self.name = name.into();
        if self.heading.is_some() {
            self.heading = Some(self.name.clone());
        }
        self.lines = lines;
        self.rows.clear();
        self.version += 1;
        self.began = self.version;
        self.left_at = None;
    }

    /// **The same document, newer** — the notice history read again: every
    /// window on it takes the newer lines and keeps its place.
    pub(crate) fn renew(&mut self, lines: Vec<PageLine>) {
        self.renew_rows(Lines {
            rows: Vec::new(),
            lines,
        });
    }

    /// The lessons this page is made of, if it is the tutor.
    pub(crate) fn lessons(&self) -> Option<&crate::tutor::Lessons> {
        match &self.recipe {
            Recipe::Tutor(lessons) => Some(lessons),
            _ => None,
        }
    }

    /// **The tutor's pictures in new colours** ([`Pictures::recolour`]) — a
    /// picture is cells, in the theme it was drawn in, so `App::set_theme`
    /// redraws them. Its text is what the page holds, a row you changed
    /// included; only what is painted changes. Nothing on any other page.
    ///
    /// [`Pictures::recolour`]: crate::tutor::Pictures::recolour
    pub(crate) fn recolour_pictures(&mut self, fresh: crate::tutor::Pictures) {
        let Recipe::Tutor(lessons) = &mut self.recipe else {
            return;
        };
        lessons.pictures.recolour(fresh);
        let text = (self.lines.iter().map(PageLine::text))
            .collect::<Vec<String>>()
            .join("\n");
        self.lines = crate::tutor::lines(&text, &lessons.pictures);
        // No line moved: said, or a window behind an edit would lose the
        // edit's shift to this version's gap (review stop 17, A L1).
        self.record(Shift::between(&text, &text));
    }

    /// [`renew`](Self::renew), each line naming its row — a listing's, and
    /// every window keeps its place by them ([`sync`](Self::sync)).
    pub(crate) fn renew_rows(&mut self, lines: Lines) {
        self.lines = lines.lines;
        self.rows = lines.rows;
        self.version += 1;
    }

    /// **A live listing derived again**: [`renew_rows`](Self::renew_rows)ed
    /// only when its lines or rows differ, so a frame that changed nothing
    /// moves no window — every window on a version it holds is left untouched
    /// (`a_frame_with_nothing_new_leaves_every_window_untouched`).
    pub(crate) fn refresh(&mut self, lines: Lines) {
        if self.lines != lines.lines || self.rows != lines.rows {
            self.renew_rows(lines);
        }
    }

    /// **The line naming `row`**, its first — where a window on the list of
    /// themes opens (on the theme worn).
    pub(crate) fn line_of(&self, row: &Row) -> Option<usize> {
        self.rows.iter().position(|r| r.as_ref() == Some(row))
    }

    /// **A new window on it**, titled for `topic` when one was asked for
    /// (`:help windows`) and for its heading otherwise, with its cursor on a
    /// line put at a place in the window — at its start without one: the
    /// top, or the end of a page read from the end.
    pub(crate) fn window(&self, topic: Option<&str>, at: Option<(usize, Place)>) -> PageWindow {
        let mut text = TextView::new(self.lines.clone());
        let end = (self.lines.len().saturating_sub(1), Place::Bottom);
        if let Some((line, place)) = at.or(self.from_the_end.then_some(end)) {
            text.reveal(line, place);
        }
        PageWindow {
            buffer: self.id,
            kind: self.kind(),
            topic: topic.or(self.heading.as_deref()).map(str::to_string),
            tags: Vec::new(),
            text,
            rows: self.rows.clone(),
            synced: self.version,
            settled: None,
        }
    }

    /// **A window on it where you last were** — that line in the middle of
    /// the window, as vim's `:b` restores a buffer's `'"` (the cursor, not
    /// the old scroll); at its start if you never were.
    pub(crate) fn returned_to(&self) -> PageWindow {
        self.window(None, self.left_at.map(|line| (line, Place::Middle)))
    }

    /// **Bring `window` up to date with it** — once per frame, for every
    /// window (`App::sync_pages`). One from before the document began starts
    /// over on the new one, as a window just opened would; one from after
    /// takes the newer lines and keeps its place — **on its rows**, where its
    /// lines name them (V5.2): the cursor on the line it was on, the window's
    /// top on the line that was at the top, wherever a row above them came or
    /// went (V4 (a)'s rule for a tree, vim's `topline`); by index where a
    /// line names nothing or its row is gone (`TextView::set_lines`).
    pub(crate) fn sync(&self, window: &mut PageWindow) {
        if window.synced == self.version {
            return;
        }
        if window.synced < self.began {
            *window = self.window(None, None);
            return;
        }
        // Painted anew, the text the same: nothing to move, and an Insert's
        // cursor keeps its place past a line's end (review stop 17, A L2).
        let same = window.text.lines().len() == self.lines.len()
            && (window.text.lines().iter().zip(&self.lines)).all(|(a, b)| a.text() == b.text());
        if same {
            window.text.restyle(self.lines.clone());
            window.rows = self.rows.clone();
            window.synced = self.version;
            return;
        }
        // An edited page's lines move as its edits moved them — the cursor
        // and the top on the text they were on, as vim's other windows
        // follow a buffer's change (`mark_adjust`): by index, an edit in one
        // tutor window put the other's cursor on another line (review stop
        // 16, A M1). A listing's by its rows.
        let shifted = self.shifts_since(window.synced);
        let moved = moves(&window.rows, &self.rows);
        window
            .text
            .set_lines_moved(self.lines.clone(), |at| match &shifted {
                Some(shifts) => Some(shifts.iter().fold(at, |line, shift| shift.map(line))),
                None => moved.get(at).copied().flatten(),
            });
        window.rows = self.rows.clone();
        window.synced = self.version;
    }
}

/// **How the most edits back a window may be and still follow its text**;
/// further back, it keeps its lines by index.
const MAX_SHIFTS: usize = 256;

/// **What one edit did to a page's lines** — the lines before it the same
/// (`prefix`), the lines after it the same (`suffix`), and how many there
/// were and are: enough to say where any line went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shift {
    prefix: usize,
    suffix: usize,
    old_len: usize,
    new_len: usize,
}

impl Shift {
    /// The shift from text `before` to `after`, compared line by line.
    fn between(before: &str, after: &str) -> Shift {
        let old: Vec<&str> = before.split('\n').collect();
        let new: Vec<&str> = after.split('\n').collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let room = old.len().min(new.len()) - prefix;
        let suffix = (old.iter().rev().zip(new.iter().rev()))
            .take(room)
            .take_while(|(a, b)| a == b)
            .count();
        Shift {
            prefix,
            suffix,
            old_len: old.len(),
            new_len: new.len(),
        }
    }

    /// Where line `line` went: before the change, where it was; after it,
    /// moved by what the change added or took; inside it, the change's
    /// first line.
    fn map(&self, line: usize) -> usize {
        if line < self.prefix {
            line
        } else if line >= self.old_len - self.suffix {
            (line + self.new_len).saturating_sub(self.old_len)
        } else {
            self.prefix.min(self.new_len.saturating_sub(1))
        }
    }
}

/// **Where a jump into the manual left from** — an entry of vim's tag
/// stack, whose `CTRL-T` is `<C-t>` here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TagFrom {
    /// A place in the manual, in this window: `(line, char)` and its title.
    Place {
        at: (usize, usize),
        topic: Option<String>,
    },
    /// Another view (the tutor, the chat), which the manual covered:
    /// coming back is `:q`'s, which gives it back.
    AnotherView,
}

/// **One window on a page buffer** — which buffer, the topic it was opened
/// at (for its title), its [`TextView`] — where you are in it — and the
/// version of the page it holds.
#[derive(Debug, Clone)]
pub(crate) struct PageWindow {
    buffer: BufferId,
    kind: PaneKind,
    /// What its title names — the topic asked for (`:help windows`), or the
    /// page's heading (`:analyze`'s command). The page is the same either
    /// way; only where it starts, and the title, differ.
    topic: Option<String>,
    /// The jumps that led here, oldest first: this window's, as in vim, and
    /// so kept when it is set aside and copied with a split.
    tags: Vec<TagFrom>,
    text: TextView,
    /// What each of `text`'s lines names — its buffer's rows at the version
    /// it holds, beside the copy of the lines it holds (empty for a page
    /// naming nothing): what it keeps its place by when the page moves on.
    rows: Vec<Option<Row>>,
    /// The version of its buffer it holds ([`PageBuffer::sync`]).
    synced: u64,
    /// The line it was at when last settled ([`moved`](Self::moved)).
    settled: Option<usize>,
}

impl PageWindow {
    /// The buffer it shows.
    pub(crate) fn buffer(&self) -> BufferId {
        self.buffer
    }

    /// The kind of page it shows — what a window set aside on it shows as
    /// when it comes back.
    pub(crate) fn kind(&self) -> PaneKind {
        self.kind
    }

    /// The pane title: ` help `, ` help — windows `, ` analyze — git status `.
    pub(crate) fn title(&self) -> String {
        self.titled(self.kind.title())
    }

    /// **The pane title, its page called `label`** — the buffer's word for
    /// itself ([`PageBuffer::label`]: ` ls! ` while `:ls` names the
    /// unlisted), with the topic this window was opened at.
    pub(crate) fn titled(&self, label: &str) -> String {
        match &self.topic {
            Some(t) => format!(" {label} — {t} "),
            None => format!(" {label} "),
        }
    }

    pub(crate) fn text(&self) -> &TextView {
        &self.text
    }

    pub(crate) fn text_mut(&mut self) -> &mut TextView {
        &mut self.text
    }

    /// **Opened by a jump** (`:help`, `<C-]>`): from another view, unless
    /// it turns out to [continue](Self::continues) a manual in this window.
    pub(crate) fn opened_by_a_jump(mut self) -> Self {
        self.tags = vec![TagFrom::AnotherView];
        self
    }

    /// **It takes over from `from`**, the window on this page it replaces:
    /// a jump within the manual, whose stack it carries on with `from`'s
    /// place on top. Nothing for a window no jump opened.
    pub(crate) fn continues(&mut self, from: &PageWindow) {
        if self.tags.is_empty() {
            return;
        }
        self.tags = from.tags.clone();
        self.tags.push(TagFrom::Place {
            at: from.text.position(),
            topic: from.topic.clone(),
        });
    }

    /// **`count` jumps back**, the stack cut there and the oldest of them
    /// returned; past the bottom, the bottom (vim's). `None` with none.
    pub(crate) fn tag_back(&mut self, count: usize) -> Option<TagFrom> {
        let at = self.tags.len().saturating_sub(count.max(1));
        self.tags.drain(at..).next()
    }

    /// **Back at a place** in the page, its title as it was there.
    pub(crate) fn return_to(&mut self, at: (usize, usize), topic: Option<String>) {
        self.text.reveal_at(at, crate::viewport::Place::Middle);
        self.topic = topic;
    }

    /// **What the line under the cursor names**, if it names anything —
    /// what `<CR>` acts on.
    pub(crate) fn row_under_cursor(&self) -> Option<&Row> {
        self.rows.get(self.text.cursor())?.as_ref()
    }

    /// **Where it is, if it moved since last asked** — its cursor, already
    /// where a jump not yet drawn lands (`TextView::reveal`) — for
    /// `App::settle_pages`, which records it as where you were in the page.
    /// A window that did not move is not where you were reading: with two
    /// on the manual, read in one and closed from the prompt, the place kept
    /// was whichever came last in the ring (full review 10-21, A 11).
    pub(crate) fn moved(&mut self) -> Option<usize> {
        let line = self.text.cursor();
        if self.settled == Some(line) {
            return None;
        }
        self.settled = Some(line);
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(n: u64) -> Option<Row> {
        Some(Row::Buffer(BufferId(n)))
    }

    /// **The tag stack**: `count` back returns the oldest of them and cuts
    /// the stack there; past the bottom, the bottom; empty, nothing.
    #[test]
    fn a_tag_stack_goes_back_by_count_and_stops_at_the_bottom() {
        let place = |line| TagFrom::Place {
            at: (line, 0),
            topic: None,
        };
        let page = PageBuffer::of(BufferId(1), Listing::Aliases.into());
        let mut window = page.window(None, None);
        window.tags = vec![TagFrom::AnotherView, place(1), place(2), place(3)];
        assert_eq!(window.tag_back(1), Some(place(3)));
        assert_eq!(window.tag_back(2), Some(place(1)));
        assert_eq!(window.tags, [TagFrom::AnotherView]);
        window.tags = vec![place(4), place(5)];
        assert_eq!(window.tag_back(9), Some(place(4)), "past it, the bottom");
        assert_eq!(window.tag_back(1), None, "and then nothing");
    }

    /// **A jump goes on with the stack; a window no jump opened keeps none**
    /// (`:messages` again replaces its window the same way).
    #[test]
    fn only_a_jump_carries_the_stack() {
        let page = PageBuffer::of(BufferId(1), Listing::Aliases.into());
        let mut from = page.window(Some("a"), None);
        from.tags = vec![TagFrom::AnotherView];

        let mut plain = page.window(None, None);
        plain.continues(&from);
        assert!(plain.tags.is_empty());

        let mut jumped = page.window(Some("b"), None).opened_by_a_jump();
        jumped.continues(&from);
        assert_eq!(
            jumped.tags,
            [
                TagFrom::AnotherView,
                TagFrom::Place {
                    at: (0, 0),
                    topic: Some("a".to_string())
                }
            ]
        );
    }

    /// **Each line goes where its row went** — the k-th line of a row to the
    /// k-th line of it (a process left running and its last line), a line
    /// naming nothing or a row gone nowhere.
    #[test]
    fn a_line_moves_with_its_row_and_its_place_in_it() {
        let left = || Some(Row::Leftover("call".into()));
        let old = [None, buffer(1), left(), left(), buffer(2)];
        let new = [None, buffer(3), buffer(1), left(), left()];
        assert_eq!(
            moves(&old, &new),
            [None, Some(2), Some(3), Some(4), None],
            "a row's two lines each to theirs; b2 gone; the heading by index"
        );
    }

    /// **A window renewed keeps its cursor on its row** — b2 under the
    /// cursor, b1's row arriving above it: still on b2.
    #[test]
    fn a_window_renewed_keeps_its_cursor_on_its_row() {
        let lines = |rows: &[Option<Row>]| {
            let mut out = Lines::default();
            for row in rows {
                out.push(row.clone(), plain(&format!("{row:?}")));
            }
            out
        };
        let mut page = PageBuffer::of(BufferId(9), Listing::Aliases.into());
        page.renew_rows(lines(&[None, buffer(2), buffer(3)]));
        let mut window = page.window(None, None);
        window.text_mut().goto_line(1, 0);
        page.renew_rows(lines(&[None, buffer(1), buffer(2), buffer(3)]));
        page.sync(&mut window);
        assert_eq!(window.row_under_cursor(), buffer(2).as_ref());
        assert_eq!(window.text().cursor(), 2);
    }
}
