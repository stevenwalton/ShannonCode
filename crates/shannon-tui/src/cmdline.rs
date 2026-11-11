//! The `:` command-line subsystem.
//!
//! One owner for state that used to be six fields spread across `App`: the
//! line buffer, command history (with vim prefix recall), Tab completion and
//! its wildmenu, and the per-line id-completion graph snapshot. Modelling it as
//! the *subsystem* (App-lifetime) rather than "an open line" is what lets
//! history live here naturally — only `line: Option<String>` tracks open vs
//! closed, and `CommandLine::open` is the single place every per-line
//! invariant (reset recall, drop the stale menu, drop the snapshot) is enforced.
//!
//! The subsystem never touches the [`Store`](shannon_core) directly: id
//! completion needs a graph, so the caller passes a `load: impl FnOnce() ->
//! Graph` and the snapshot is filled at most once per open line — keeping the
//! "walk a loaded graph, don't call the Store per frame" posture without
//! coupling this module to persistence.

use crossterm::event::{KeyCode, KeyEvent};
use shannon_core::Graph;

use crate::command::{Completion, IdSpace, Supplied};

/// Minimum hex length for a completed short-id (git's short-hash convention).
/// Completion offers at least this many chars, more once you've typed past it.
///
/// The shared constant, not a local `8`: `EpisodeId::short_hex` used to call
/// itself "the one canonical short-id" while this file quietly held a second
/// copy of the number.
const SHORT_ID: usize = shannon_core::SHORT_ID_LEN;

/// An open Tab-completion menu: the fixed left part of the line, the candidate
/// words, and which one is currently applied. Vim calls the on-screen list a
/// *wildmenu*. Never constructed empty (no candidates ⇒ no menu), so
/// `candidates[selected]` is always in bounds.
pub(crate) struct Wildmenu {
    /// Fixed left part of the line (everything before the completed word).
    stem: String,
    /// Candidate words to cycle through (non-empty).
    candidates: Vec<String>,
    /// Index of the currently-applied candidate.
    selected: usize,
}

/// A cell count as a screen column, saturating rather than wrapping — a
/// silent wrap would put the cursor on a wildly wrong cell rather than at the
/// edge.
fn saturating_col(cells: usize) -> u16 {
    u16::try_from(cells).unwrap_or(u16::MAX)
}

/// **What the open line is for** — a `:` command, or a `/` (forward) or `?`
/// search. One line, as vim's: the same editing keys, cursor and recall;
/// what Enter does with the text is the caller's, by this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LineKind {
    #[default]
    Command,
    Search {
        forward: bool,
    },
}

impl LineKind {
    /// The char the line is drawn after: `:`, `/` or `?`.
    pub(crate) fn glyph(self) -> char {
        match self {
            LineKind::Command => ':',
            LineKind::Search { forward: true } => '/',
            LineKind::Search { forward: false } => '?',
        }
    }
}

/// The `:` command line: buffer + history + completion menu + id snapshot.
#[derive(Default)]
pub(crate) struct CommandLine {
    /// `Some(typed)` while the line is open, `None` otherwise — the only bit
    /// that models open vs closed.
    line: Option<String>,
    /// What the line is for — set by each way of opening it.
    kind: LineKind,
    /// The patterns searched, oldest first — vim keeps them apart from the
    /// commands, so `/`↑ recalls a pattern and `:`↑ a command.
    searches: Vec<String>,
    /// **Where an edit lands**, as a byte offset into `line`.
    ///
    /// The line had no cursor at all: every key appended and `Backspace`
    /// popped, so `<Left>`/`<Right>`/`<Home>`/`<End>` were dead and the only
    /// way to fix a typo near the start was to delete everything after it.
    /// Steven hit it editing an `:analyze` command (2026-08-25) — and hit the
    /// same missing concept from the other side, because a line with no cursor
    /// has nothing to scroll *to*, so a command wider than the row was simply
    /// invisible past the edge.
    ///
    /// Always on a char boundary: every write goes through this module, and
    /// the moves step by `char`, never by byte
    /// ([[feedback_char_boundary_slicing]]).
    cursor: usize,
    /// Executed commands, oldest first — vim's command-line history, recalled
    /// with ↑/↓. An empty one isn't recorded, and one typed again moves to the
    /// end (vim's: each command once). Outlives an individual line
    /// (session-scoped; not yet persisted across launches).
    history: Vec<String>,
    /// While browsing history: the index into `history` currently shown (`None`
    /// = editing a fresh line, not browsing).
    history_pos: Option<usize>,
    /// The line typed before history browsing began, stashed so ↓ past the
    /// newest entry restores it (vim's behaviour).
    draft: String,
    /// The open completion menu (`Some` between the first `<Tab>` and the next
    /// edit); `<Tab>`/`<S-Tab>` cycle it, any edit drops it.
    menu: Option<Wildmenu>,
    /// Graph snapshot for id completion, loaded once per open line (via the
    /// caller's loader) and dropped by [`open`](Self::open). The ids offered are
    /// frozen for the line's lifetime; the command reloads a fresh graph when it
    /// executes, so staleness is only cosmetic.
    graph: Option<Graph>,
}

impl CommandLine {
    /// Open the `:` line fresh (empty) and reset **all** per-line state — recall
    /// position, the stale completion menu, and the id snapshot. The single
    /// entry point every way in (Normal `:`, or `:` from `:graph`) passes
    /// through, so these invariants live in one place.
    pub(crate) fn open(&mut self) {
        self.line = Some(String::new());
        self.kind = LineKind::Command;
        self.cursor = 0;
        self.history_pos = None;
        self.menu = None;
        self.graph = None;
    }

    /// Open the line for a search — `/` (`forward`) or `?` — through
    /// [`open`](Self::open), so the per-line resets hold here too.
    pub(crate) fn open_search(&mut self, forward: bool) {
        self.open();
        self.kind = LineKind::Search { forward };
    }

    /// What the open (or last) line is for.
    pub(crate) fn kind(&self) -> LineKind {
        self.kind
    }

    /// The history this line recalls from and records to.
    fn history_mut(&mut self) -> &mut Vec<String> {
        match self.kind {
            LineKind::Command => &mut self.history,
            LineKind::Search { .. } => &mut self.searches,
        }
    }

    /// [`history_mut`](Self::history_mut), for reading.
    fn history(&self) -> &[String] {
        match self.kind {
            LineKind::Command => &self.history,
            LineKind::Search { .. } => &self.searches,
        }
    }

    /// Open the line already holding `text`, cursor at its end — a command
    /// offered for editing (the decision pane's `A` opens `:allow <rule>`).
    /// Through [`open`](Self::open) and [`set_line`](Self::set_line), so the
    /// per-line resets and the cursor rule hold here too.
    pub(crate) fn open_with(&mut self, text: &str) {
        self.open();
        self.set_line(text.to_string());
    }

    /// **Insert a paste at the cursor**, as typing it would, in one step.
    ///
    /// The line had no way in for a paste at all, so a regex rule had to be
    /// typed by hand (Steven, 2026-10-03). The line is ONE line: a paste
    /// holding a line break is refused whole — inserted, the break would
    /// either run what came before it or join two lines into a command
    /// nobody wrote. The break that ends a copied line is not one of those,
    /// and is dropped. A tab is a space here, where every column is a cell.
    pub(crate) fn paste(&mut self, text: &str) -> Result<(), String> {
        let text = text.trim_end_matches('\n');
        let lines = text.lines().count();
        if lines > 1 {
            return Err(format!(
                "the `:` line is one line, and the paste holds {lines} — nothing was pasted"
            ));
        }
        let Some(line) = self.line.as_mut() else {
            return Err("the `:` line is not open".into());
        };
        let text = text.replace('\t', " ");
        self.menu = None;
        line.insert_str(self.cursor, &text);
        self.cursor += text.len();
        Ok(())
    }

    /// Abandon the line without running it — what `Esc` does, reachable by a
    /// caller that is not routing a key (`CTRL-C`).
    pub(crate) fn cancel(&mut self) {
        self.line = None;
        self.cursor = 0;
        self.menu = None;
    }

    /// Whether the `:` line is currently open.
    pub(crate) fn is_open(&self) -> bool {
        // Built on `line()` rather than on the field, so the two cannot come to
        // disagree about what "open" means — and so `line()` keeps a production
        // caller now that the renderer reads `window()` instead of the raw text.
        self.line().is_some()
    }

    /// Whether the line is open with its cursor after its last character —
    /// neovim's `c` cursor rather than its `ci` (`cmdline_at_end`).
    pub(crate) fn at_end(&self) -> bool {
        self.line().is_some_and(|line| self.cursor == line.len())
    }

    /// The current line text while open, else `None`.
    pub(crate) fn line(&self) -> Option<&str> {
        self.line.as_deref()
    }

    /// The open completion menu for the renderer — `(candidates, selected)`
    /// while `<Tab>` cycling is active, else `None`.
    pub(crate) fn menu(&self) -> Option<(&[String], usize)> {
        self.menu
            .as_ref()
            .map(|m| (m.candidates.as_slice(), m.selected))
    }

    /// **Replace the whole line and put the cursor at its end.**
    ///
    /// The one door for a wholesale replacement — history recall, and each of
    /// the three completion paths. Five sites wrote `self.line = Some(..)`
    /// directly, and every one of them would have had to remember the cursor
    /// separately; a cursor left past the end of a shorter line is a byte
    /// offset that is no longer a char boundary, which is a panic rather than a
    /// glitch ([[feedback_char_boundary_slicing]]).
    fn set_line(&mut self, text: String) {
        self.cursor = text.len();
        self.line = Some(text);
    }

    /// The byte offset one character left of `at`, or `at` at the start.
    fn left_of(line: &str, at: usize) -> usize {
        line[..at]
            .chars()
            .next_back()
            .map_or(at, |c| at - c.len_utf8())
    }

    /// The byte offset one character right of `at`, or `at` at the end.
    fn right_of(line: &str, at: usize) -> usize {
        line[at..].chars().next().map_or(at, |c| at + c.len_utf8())
    }

    /// Start of the word before `at` — where `<C-w>` cuts back to, as vim's
    /// `c_CTRL-W` (`crate::word::start_before`): the blanks, then one run of
    /// a class. A byte offset, as the line's are.
    fn word_start(line: &str, at: usize) -> usize {
        let head: Vec<char> = line[..at].chars().collect();
        let start = crate::word::start_before(&head, head.len());
        head[..start].iter().map(|c| c.len_utf8()).sum()
    }

    /// **What to draw, and where the cursor sits in it**, for a row `width`
    /// cells wide: the visible slice of the line, and the cursor's column
    /// within that slice.
    ///
    /// Stateless — the window is derived from the cursor every frame rather
    /// than stored, so there is no scroll offset that can disagree with where
    /// the cursor actually is. Measured in **cells**, not chars, because a
    /// command can carry a path with wide characters in it.
    ///
    /// The rule is vim's: while the line fits, show all of it; once it does
    /// not, scroll so the cursor stays on screen.
    ///
    /// **Walked out from the cursor, a cell at a time**, so a frame costs the
    /// row's width, not the line's. It walked forward from the line's start,
    /// re-measuring the whole slice at every step: quadratic, per frame, and
    /// a 32k-character paste took seven seconds a frame (review 20, Reviewer
    /// B). Back from the cursor while the cells between still leave it a
    /// column, then on from it while the row holds more — the same slice,
    /// since a width is a sum over characters (`wrap::str_cells`).
    pub(crate) fn window(&self, width: u16) -> (&str, u16) {
        use crate::wrap::char_cells;
        let Some(line) = self.line.as_deref() else {
            return ("", 0);
        };
        let width = usize::from(width);
        let (head, tail) = line.split_at(self.cursor);
        let (mut start, mut used) = (self.cursor, 0);
        for c in head.chars().rev() {
            if used + char_cells(c) >= width {
                break;
            }
            used += char_cells(c);
            start -= c.len_utf8();
        }
        let col = saturating_col(used);
        let mut end = self.cursor;
        for c in tail.chars() {
            if used + char_cells(c) >= width {
                break;
            }
            used += char_cells(c);
            end += c.len_utf8();
        }
        (&line[start..end], col)
    }

    /// Is this key carrying a modifier that makes it a chord rather than text?
    ///
    /// `SHIFT` is not one: a shifted letter IS the letter you meant.
    fn is_chord(key: &KeyEvent) -> bool {
        key.modifiers.intersects(
            crossterm::event::KeyModifiers::CONTROL
                | crossterm::event::KeyModifiers::ALT
                | crossterm::event::KeyModifiers::SUPER,
        )
    }

    /// Edit the open `:` line with `key` (no I/O — testable). Returns
    /// `Some(command)` when `Enter` commits it (the line is then closed and the
    /// command recorded in history); `Esc`, or `Backspace` past the `:`, closes
    /// the line and returns `None`. ↑/↓ recall history; `<Tab>`/`<S-Tab>`
    /// complete. `load` supplies the id-completion graph on demand (only Tab on
    /// an id argument calls it).
    ///
    /// Test-only: the app supplies words of its own too, through
    /// [`input_with`](Self::input_with); this is that with none.
    #[cfg(test)]
    pub(crate) fn input<F: FnOnce() -> Graph>(&mut self, key: KeyEvent, load: F) -> Option<String> {
        self.input_with(key, load, |_, _| Vec::new())
    }

    /// [`input`](Self::input), with the words only the app holds for an
    /// argument that completes from them: `supplied(source, prefix)` returns
    /// the matches from that [`Supplied`] source, in preference order (typed
    /// text: the prompt's, then the chat's, latest first). Only Tab on such an
    /// argument calls it.
    pub(crate) fn input_with<F, T>(&mut self, key: KeyEvent, load: F, supplied: T) -> Option<String>
    where
        F: FnOnce() -> Graph,
        T: FnOnce(Supplied, &str) -> Vec<String>,
    {
        self.line.as_ref()?;
        // Any key but Tab/Shift-Tab edits or ends the line, so a live completion
        // menu is stale — drop it. (Tab handling reads it before this fires.)
        if !matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            self.menu = None;
        }
        match key.code {
            KeyCode::Esc => {
                self.line = None;
                self.cursor = 0;
            }
            // A pattern completes nothing: the commands' words are not its.
            KeyCode::Tab | KeyCode::BackTab if self.kind != LineKind::Command => {}
            // Tab cycles command / argument completion; Shift-Tab reverses.
            KeyCode::Tab => self.complete(1, load, supplied),
            KeyCode::BackTab => self.complete(-1, load, supplied),
            KeyCode::Enter => {
                let cmd = self.line.take().expect("checked Some");
                // Record it, as vim does: an empty one not at all, and one
                // typed again moves to the end, its older entry dropped —
                // measured, Neovim 0.12.6: `:echo 1` `:echo 2` `:echo 1`
                // keeps `echo 2`, `echo 1`. This kept consecutive repeats
                // out only, and called that vim's (review stop 12, T6 12).
                // Unlike vim, it is kept trimmed: recalled, it is the
                // command, not the blanks typed around it.
                let trimmed = cmd.trim().to_string();
                let history = self.history_mut();
                if !trimmed.is_empty() {
                    history.retain(|was| *was != trimmed);
                    history.push(trimmed);
                }
                self.history_pos = None;
                self.cursor = 0;
                return Some(cmd);
            }
            KeyCode::Up => self.recall(-1),
            KeyCode::Down => self.recall(1),
            // Movement. `<Home>`/`<End>` and vim's `c_CTRL-B`/`c_CTRL-E` are
            // the same two positions under two names, so they share an arm.
            KeyCode::Left => {
                let buf = self.line.as_ref().expect("checked Some");
                self.cursor = Self::left_of(buf, self.cursor);
            }
            KeyCode::Right => {
                let buf = self.line.as_ref().expect("checked Some");
                self.cursor = Self::right_of(buf, self.cursor);
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::Char('b') if Self::is_chord(&key) => self.cursor = 0,
            KeyCode::End => self.cursor = self.line.as_ref().expect("checked Some").len(),
            KeyCode::Char('e') if Self::is_chord(&key) => {
                self.cursor = self.line.as_ref().expect("checked Some").len();
            }
            // Backspace past the `:` (empty line) leaves command mode (vim).
            // At the START of a non-empty line it does nothing — vim only
            // leaves when there is nothing left to delete.
            KeyCode::Backspace => {
                let buf = self.line.as_mut().expect("checked Some");
                if buf.is_empty() {
                    self.line = None;
                    self.cursor = 0;
                } else if self.cursor > 0 {
                    let from = Self::left_of(buf, self.cursor);
                    buf.replace_range(from..self.cursor, "");
                    self.cursor = from;
                }
            }
            KeyCode::Delete => {
                let buf = self.line.as_mut().expect("checked Some");
                let to = Self::right_of(buf, self.cursor);
                buf.replace_range(self.cursor..to, "");
            }
            // **A chord is not its letter.** This matched `key.code` alone, so
            // `<C-w>` typed a `w` and `<C-u>` typed a `u` onto the command line
            // — the modifier-blind class this project keeps finding
            // (`[[feedback_teardown_doors]]`), and a vim-fidelity gap, since
            // both are real command-line editing keys there (`c_CTRL-W`,
            // `c_CTRL-U`). They do here what vim does: kill the word before the
            // cursor, kill the whole line. Everything else modified is ignored
            // rather than typed.
            KeyCode::Char('w') if Self::is_chord(&key) => {
                let buf = self.line.as_mut().expect("checked Some");
                let cut = Self::word_start(buf, self.cursor);
                buf.replace_range(cut..self.cursor, "");
                self.cursor = cut;
            }
            // Vim's `c_CTRL-U` kills what is BEFORE the cursor, not the whole
            // line — the distinction did not exist while there was no cursor.
            KeyCode::Char('u') if Self::is_chord(&key) => {
                let buf = self.line.as_mut().expect("checked Some");
                buf.replace_range(..self.cursor, "");
                self.cursor = 0;
            }
            KeyCode::Char(c) if !Self::is_chord(&key) => {
                let buf = self.line.as_mut().expect("checked Some");
                buf.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            _ => {}
        }
        None
    }

    /// Recall command history: `dir < 0` is ↑ (older), `dir > 0` is ↓ (newer).
    /// Vim behaviour: the first ↑ stashes what you've typed as both the restore
    /// **draft** and a **prefix filter** — ↑/↓ then walk only entries starting
    /// with that prefix (an empty line matches everything). ↓ past the newest
    /// match restores the draft.
    fn recall(&mut self, dir: i32) {
        if self.line.is_none() || self.history().is_empty() {
            return;
        }
        // Begin browsing on the first ↑: capture the typed line as draft + prefix.
        if self.history_pos.is_none() {
            if dir > 0 {
                return; // ↓ when not browsing: nothing to do
            }
            self.draft = self.line.clone().unwrap_or_default();
        }
        let history = self.history();
        let len = history.len();
        let prefix = self.draft.clone();
        let next = if dir < 0 {
            // Older: the closest match below the current position.
            let start = self.history_pos.unwrap_or(len);
            (0..start).rev().find(|&i| history[i].starts_with(&prefix))
        } else {
            // Newer: the closest match above; none left → fall back to the draft.
            let start = self.history_pos.map_or(len, |p| p + 1);
            (start..len).find(|&i| history[i].starts_with(&prefix))
        };
        match next {
            Some(i) => {
                let line = history[i].clone();
                self.history_pos = Some(i);
                self.set_line(line);
            }
            // Stepped past the newest match → back to the typed draft.
            None if dir > 0 => {
                self.history_pos = None;
                self.set_line(prefix);
            }
            // ↑ with no older match → stay put.
            None => {}
        }
    }

    /// Advance Tab-completion by `dir` (`+1` = `<Tab>`, forward; `-1` = `<S-Tab>`,
    /// backward). The first press computes candidates with
    /// [`command::complete`](crate::command::complete); later presses cycle the
    /// open menu. A unique match completes in place (no menu); no match is a
    /// no-op. The line is rewritten to `stem + candidate`.
    fn complete<F, T>(&mut self, dir: i32, load: F, supplied: T)
    where
        F: FnOnce() -> Graph,
        T: FnOnce(Supplied, &str) -> Vec<String>,
    {
        // Cycling an already-open menu: step `selected` (wrapping) and rewrite.
        if let Some(menu) = &mut self.menu {
            let n = menu.candidates.len() as i32;
            menu.selected = (menu.selected as i32 + dir).rem_euclid(n) as usize;
            let next = format!("{}{}", menu.stem, menu.candidates[menu.selected]);
            self.set_line(next);
            return;
        }
        // Fresh completion from the current line.
        let Some(line) = self.line.clone() else {
            return;
        };
        let Completion {
            stem,
            word,
            mut candidates,
            ids,
            supplied: source,
        } = crate::command::complete(&line);
        // Fold live graph ids in after the fixed words, per the argument's space.
        candidates.extend(self.matching_short_ids(ids, &word, load));
        // And the app's own words, in the app's order, for an argument drawn
        // from them.
        if source != Supplied::None {
            candidates.extend(supplied(source, &word));
        }
        match candidates.len() {
            // Nothing matches — leave the line as typed (vim rings the bell; we
            // just no-op).
            0 => {}
            // Unique match: fill it, with no menu to cycle.
            1 => self.set_line(format!("{stem}{}", candidates[0])),
            // Several: apply one end (last when reversing) and open the wildmenu.
            _ => {
                let selected = if dir < 0 { candidates.len() - 1 } else { 0 };
                self.set_line(format!("{stem}{}", candidates[selected]));
                self.menu = Some(Wildmenu {
                    stem,
                    candidates,
                    selected,
                });
            }
        }
    }

    /// Live graph short-ids matching `word` (a hex prefix), for the given id
    /// space: [`IdSpace::Episodes`] yields episode ids only, [`IdSpace::Nodes`]
    /// yields episodes **and** turns, [`IdSpace::None`] yields nothing. Short-ids
    /// are ≥ [`SHORT_ID`] chars (longer once you've typed past that), episodes
    /// before turns, each group sorted + de-duplicated (content-addressed turns
    /// recur across branches), and a turn short already shown as an episode is
    /// dropped. `load` builds the graph snapshot, called at most once per line.
    ///
    /// **Session names come first** — what a person types — matched as they
    /// are read, exactly (a name is case-sensitive; an id is not).
    pub(crate) fn matching_short_ids<F: FnOnce() -> Graph>(
        &mut self,
        ids: IdSpace,
        word: &str,
        load: F,
    ) -> Vec<String> {
        let want_turns = match ids {
            IdSpace::None => return Vec::new(),
            IdSpace::Episodes => false,
            IdSpace::Nodes => true,
        };
        let typed = word;
        let word = word.to_lowercase();
        // Load the store-backed graph once per `:` line (see the `graph` field).
        let graph = self.graph.get_or_insert_with(load);
        let mut names: Vec<String> = graph
            .episode_ids()
            .iter()
            .filter_map(|e| graph.episode(e)?.label.name.clone())
            .filter(|n| n.starts_with(typed))
            .collect();
        names.sort();
        // Collect the matching hexes, trim each to a short id no shorter than
        // what's been typed, then sort + dedup.
        let sorted_shorts = |hexes: &mut dyn Iterator<Item = String>| -> Vec<String> {
            let mut v: Vec<String> = hexes
                .filter(|h| h.starts_with(&word))
                .map(|h| h[..word.len().max(SHORT_ID).min(h.len())].to_string())
                .collect();
            v.sort();
            v.dedup();
            v
        };

        let mut episodes = sorted_shorts(&mut graph.episode_ids().iter().map(|e| e.to_hex()));
        let turns = if want_turns {
            sorted_shorts(
                &mut graph
                    .episode_ids()
                    .iter()
                    .filter_map(|e| graph.episode(e))
                    // Each turn id is an own turn of exactly one episode (a shared
                    // turn belongs to the episode that first added it), so this
                    // enumerates every unique turn once for completion.
                    .flat_map(|ep| {
                        ep.own_turns()
                            .into_iter()
                            .map(|t| t.to_hex())
                            .collect::<Vec<_>>()
                    }),
            )
        } else {
            Vec::new()
        };

        // Episodes first; drop any turn short already shown as an episode
        // (`episodes` is still sorted here, so binary-search — no separate set).
        let unique_turns: Vec<String> = turns
            .into_iter()
            .filter(|t| episodes.binary_search(t).is_err())
            .collect();
        episodes.extend(unique_turns);
        names.extend(episodes);
        names
    }

    /// Whether the id-completion snapshot is currently loaded (test-facing — the
    /// cache is otherwise invisible).
    #[cfg(test)]
    pub(crate) fn snapshot_loaded(&self) -> bool {
        self.graph.is_some()
    }
}

/// How many words typed-text completion offers — a menu, not a search result.
const TYPED_WORDS: usize = 50;

/// **Words already typed that start with `prefix`**, for completing an
/// argument drawn from them (`:secret hun<Tab>`): the prompt's first, in
/// reading order, then the chat's **from the end back** — the most recent
/// mention first, and the search stops once the menu is full, so a long
/// transcript is not walked to its start (Steven, 2026-10-01).
///
/// **Every way a word could end** is offered, shortest first: punctuation at
/// its edges may or may not be part of a secret, and only the user knows.
/// `"hunter2!",` gives `hunter2`, `hunter2!`, `hunter2!"`, `hunter2!",` —
/// then `"hunter2!",` as written, for a secret that STARTS with punctuation.
/// Trimming both ends (the first version) made `hunter2!` unreachable, and
/// marking `hunter2` for that password leaves the `!` on the wire. A plain
/// `hunter2,` gives two. `skip` drops words not worth offering (marked).
pub(crate) fn typed_words<'a>(
    prompt: &'a str,
    chat_from_end: impl Iterator<Item = &'a str>,
    prefix: &str,
    skip: impl Fn(&str) -> bool,
) -> Vec<String> {
    let edge = |c: char| "\"'`,.;:!?()[]{}<>".contains(c);
    let forms = move |w: &'a str| {
        let lead = w.trim_start_matches(edge);
        let core = lead.trim_end_matches(edge);
        let tail = &lead[core.len()..];
        let ends = tail
            .char_indices()
            .map(move |(i, c)| &lead[..core.len() + i + c.len_utf8()]);
        std::iter::once(core).chain(ends).chain([w])
    };
    let words = prompt.split_whitespace().flat_map(forms).chain(
        chat_from_end
            .flat_map(|line| line.split_whitespace().rev())
            .flat_map(forms),
    );
    let mut out: Vec<String> = Vec::new();
    for w in words {
        if out.len() == TYPED_WORDS {
            break;
        }
        if w.is_empty() || !w.starts_with(prefix) || skip(w) || out.iter().any(|o| o == w) {
            continue;
        }
        out.push(w.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The order Steven asked for: the prompt first, then the chat from the
    /// latest line back; a word in punctuation offered with every end it
    /// could have; duplicates once; `skip` honoured; the menu bounded.
    #[test]
    fn typed_words_prefer_the_prompt_then_the_latest_chat() {
        let chat = [
            "early: hunter-old",
            "later: hunter-new",
            "last \"hunter2!\",",
        ];
        let got = typed_words(
            "use hunter-prompt please",
            chat.iter().rev().copied(),
            "hunter",
            |w| w == "hunter-old",
        );
        assert_eq!(
            got,
            vec![
                "hunter-prompt",
                "hunter2",
                "hunter2!",
                "hunter2!\"",
                "hunter2!\",",
                "hunter-new",
            ],
            "prompt, then latest chat first; every end; hunter-old skipped"
        );
        // A secret that starts with punctuation is reachable as written.
        assert_eq!(
            typed_words("pw: !bang-word", std::iter::empty(), "!b", |_| false),
            vec!["!bang-word"]
        );
        let many = (0..200)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            typed_words(&many, std::iter::empty(), "w", |_| false).len(),
            TYPED_WORDS,
            "bounded"
        );
    }
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use shannon_core::Graph;

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn plain(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn typed(text: &str) -> CommandLine {
        let mut cl = CommandLine::default();
        cl.open();
        for c in text.chars() {
            cl.input(plain(c), Graph::new);
        }
        cl
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// **A command typed again moves to the end of the history** — vim's
    /// rule, each case measured in Neovim 0.12.6 (`histget(':', i)` after the
    /// commands, typed): the older entry goes, wherever it was.
    #[test]
    fn history_keeps_each_command_once_at_its_latest() {
        for (typed_in, kept) in [
            (
                &["echo 1", "echo 2", "echo 1"][..],
                &["echo 2", "echo 1"][..],
            ),
            (&["echo 1", "echo 1"], &["echo 1"]),
            (
                &["echo 1", "echo 2", "echo 3", "echo 2"],
                &["echo 1", "echo 3", "echo 2"],
            ),
        ] {
            let mut cl = CommandLine::default();
            for command in typed_in {
                cl.open();
                for c in command.chars() {
                    cl.input(plain(c), Graph::new);
                }
                cl.input(key(KeyCode::Enter), Graph::new);
            }
            assert_eq!(cl.history(), kept, "{typed_in:?}");
        }
    }

    /// **A search line is the same line with its own history, and completes
    /// nothing**: `/`↑ recalls a pattern, `:`↑ a command, and <Tab> in a
    /// pattern leaves it as typed (the commands' words are not its).
    #[test]
    fn a_search_line_keeps_its_own_history_and_completes_nothing() {
        let mut cl = CommandLine::default();
        cl.open_search(true);
        assert_eq!(cl.kind().glyph(), '/');
        // `hel` is the start of one command (`:help`), which Tab completes
        // on a `:` line.
        for c in "hel".chars() {
            cl.input(plain(c), Graph::new);
        }
        cl.input(key(KeyCode::Tab), Graph::new);
        assert_eq!(cl.line(), Some("hel"), "no completion in a pattern");
        assert_eq!(
            cl.input(key(KeyCode::Enter), Graph::new).as_deref(),
            Some("hel")
        );
        cl.open();
        cl.input(key(KeyCode::Up), Graph::new);
        assert_eq!(cl.line(), Some(""), "a command's history holds no pattern");
        cl.open_search(false);
        assert_eq!(cl.kind().glyph(), '?');
        cl.input(key(KeyCode::Up), Graph::new);
        assert_eq!(cl.line(), Some("hel"), "a search's holds it");
    }

    /// **The line has a cursor**, so `<Left>`/`<Right>`/`<Home>`/`<End>` move
    /// and an insert lands where you put it.
    ///
    /// It did not: every key appended and `Backspace` popped, so the only way
    /// to fix a typo near the start of a long `:analyze` command was to delete
    /// everything after it (Steven, 2026-08-25).
    #[test]
    fn the_cursor_moves_and_edits_land_on_it() {
        let mut cl = typed("analyze ls");
        cl.input(key(KeyCode::Left), Graph::new);
        cl.input(key(KeyCode::Left), Graph::new);
        cl.input(plain('X'), Graph::new);
        assert_eq!(cl.line(), Some("analyze Xls"));

        cl.input(key(KeyCode::Home), Graph::new);
        cl.input(plain('!'), Graph::new);
        assert_eq!(cl.line(), Some("!analyze Xls"));

        cl.input(key(KeyCode::End), Graph::new);
        cl.input(plain('?'), Graph::new);
        assert_eq!(cl.line(), Some("!analyze Xls?"));

        // Backspace takes the char BEFORE the cursor, not the last one typed.
        cl.input(key(KeyCode::Home), Graph::new);
        cl.input(key(KeyCode::Right), Graph::new);
        cl.input(key(KeyCode::Backspace), Graph::new);
        assert_eq!(cl.line(), Some("analyze Xls?"));

        // Delete takes the char AT the cursor.
        cl.input(key(KeyCode::Delete), Graph::new);
        assert_eq!(cl.line(), Some("nalyze Xls?"));
    }

    /// **A paste lands at the cursor, whole**, and never runs a line: Steven
    /// typed a regex rule by hand because the line took no paste. A copied
    /// line's own break is dropped; two lines are refused, the line kept.
    #[test]
    fn a_paste_lands_at_the_cursor_and_never_runs_a_line() {
        let mut cl = typed("deny ");
        cl.paste("re:^(?:.*/|\\\\)?rm(?:\\s|$)\n").unwrap();
        assert_eq!(cl.line(), Some(r"deny re:^(?:.*/|\\)?rm(?:\s|$)"));

        // At the cursor, which lands after it on a character boundary.
        cl.input(key(KeyCode::Home), Graph::new);
        cl.paste("é\t").unwrap();
        cl.input(plain('!'), Graph::new);
        assert_eq!(cl.line(), Some(r"é !deny re:^(?:.*/|\\)?rm(?:\s|$)"));

        let kept = cl.line().map(str::to_string);
        let err = cl.paste("ls\nrm -rf ~\n").unwrap_err();
        assert!(err.contains("holds 2"), "{err}");
        assert_eq!(cl.line().map(str::to_string), kept, "nothing was pasted");

        assert!(CommandLine::default().paste("x").is_err(), "a closed line");
    }

    /// Moves and deletes step by **character**, so a multibyte line cannot land
    /// an offset mid-char — which is a panic, not a glitch, and ends the
    /// session ([[feedback_char_boundary_slicing]]).
    #[test]
    fn the_cursor_never_lands_mid_character() {
        let mut cl = typed("日本語");
        cl.input(key(KeyCode::Left), Graph::new);
        cl.input(plain('x'), Graph::new);
        assert_eq!(cl.line(), Some("日本x語"));
        cl.input(key(KeyCode::Backspace), Graph::new);
        cl.input(key(KeyCode::Backspace), Graph::new);
        assert_eq!(cl.line(), Some("日語"));
        // Past either end is a no-op, not an underflow.
        for _ in 0..8 {
            cl.input(key(KeyCode::Left), Graph::new);
        }
        cl.input(plain('a'), Graph::new);
        assert_eq!(cl.line(), Some("a日語"));
    }

    /// `window` keeps the cursor on screen. Without it a long command simply
    /// ran off the right edge with nothing to say so — you could not read the
    /// command you were about to have analysed.
    #[test]
    fn a_line_too_long_for_the_row_scrolls_to_the_cursor() {
        let mut cl = typed("analyze abcdefghijklmnopqrstuvwxyz");
        // Fits: all of it, cursor at the end.
        let (shown, col) = cl.window(80);
        assert_eq!(shown, "analyze abcdefghijklmnopqrstuvwxyz");
        assert_eq!(usize::from(col), shown.chars().count());

        // Does not fit: the cursor (at the end) must still be inside the row.
        let (shown, col) = cl.window(10);
        assert!(
            shown.ends_with('z'),
            "the tail is what you are typing: {shown:?}"
        );
        assert!(usize::from(col) < 10, "cursor stayed on the row: {col}");
        assert!(
            crate::wrap::str_cells(shown) < 10,
            "and the slice fits: {shown:?}"
        );

        // Move home and the window follows back to the head.
        cl.input(key(KeyCode::Home), Graph::new);
        let (shown, col) = cl.window(10);
        assert!(shown.starts_with("analyze"), "{shown:?}");
        assert_eq!(col, 0);
    }

    /// **A one-column terminal must not end the session.**
    ///
    /// The forward walk this replaced needed `width.max(1)` to hold: with the
    /// cursor mid-line, `window(0)` panicked on `begin > end` while slicing,
    /// and at the end it **never returned** (Reviewer B, by deleting it). On
    /// the render path, reachable by squeezing a split to one column with `:`
    /// open. The walk out from the cursor steps over characters, so no width
    /// can stall it; this holds that it stays so.
    #[test]
    fn a_row_with_no_room_yields_nothing_rather_than_hanging() {
        for text in ["", "a", "abc", "日本語", "analyze ls -la"] {
            for cursor_left in 0..4 {
                let mut cl = typed(text);
                for _ in 0..cursor_left {
                    cl.input(key(KeyCode::Left), Graph::new);
                }
                for width in [0u16, 1, 2] {
                    let (shown, col) = cl.window(width);
                    assert!(
                        crate::wrap::str_cells(shown) <= usize::from(width.max(1)),
                        "{text:?} at width {width}: {shown:?}"
                    );
                    assert!(text.contains(shown), "the slice is part of the line");
                    let _ = col;
                }
            }
        }
    }

    /// **The walk out from the cursor is the slice the forward walk drew** —
    /// that one kept here as the oracle, over lines mixing narrow, wide and
    /// zero-width characters, every cursor, every width that matters.
    #[test]
    fn window_is_the_slice_the_forward_walk_drew() {
        fn oracle(line: &str, cursor: usize, width: u16) -> (&str, u16) {
            let cells = crate::wrap::str_cells;
            let width = usize::from(width.max(1));
            if cells(line) < width {
                return (line, saturating_col(cells(&line[..cursor])));
            }
            let mut start = 0;
            while cells(&line[start..cursor]) >= width {
                start = CommandLine::right_of(line, start);
            }
            let mut end = start;
            while end < line.len() {
                let next = CommandLine::right_of(line, end);
                if cells(&line[start..next]) >= width {
                    break;
                }
                end = next;
            }
            (
                &line[start..end],
                saturating_col(cells(&line[start..cursor])),
            )
        }
        for text in [
            "analyze ls -la /tmp",
            "a日b本c語d",
            "x\u{301}y\u{200b}z日本",
        ] {
            let mut cl = typed(text);
            let line = text.to_string();
            loop {
                for width in 1..=u16::try_from(line.len() + 2).expect("short") {
                    assert_eq!(
                        cl.window(width),
                        oracle(&line, cl.cursor, width),
                        "{text:?} cursor {} width {width}",
                        cl.cursor
                    );
                }
                if cl.cursor == 0 {
                    break;
                }
                cl.input(key(KeyCode::Left), Graph::new);
            }
        }
    }

    /// `window` measures **cells, not chars** — the unit error `elide_title` and
    /// `fit_segments` each shipped once. Replacing every `str_cells` inside it
    /// with `chars().count()` left 1017 tests green, because the only two
    /// callers were an ASCII fixture and a multibyte test that never called
    /// `window` (Reviewer C).
    #[test]
    fn window_measures_cells_not_characters() {
        // Eight wide chars = 16 cells. A char-count budget would think all
        // eight fit a 10-cell row.
        let cl = typed("日本語日本語日本");
        let (shown, col) = cl.window(10);
        assert!(
            crate::wrap::str_cells(shown) < 10,
            "measured in cells: {shown:?} is {} cells",
            crate::wrap::str_cells(shown)
        );
        assert!(
            shown.chars().count() < 8,
            "…so fewer than eight of them fit: {shown:?}"
        );
        assert!(usize::from(col) < 10, "and the cursor is on the row: {col}");
    }

    /// A wholesale replacement — history recall, completion — leaves the cursor
    /// at the end of the NEW line. A cursor past the end of a shorter line is
    /// an offset that is no longer a char boundary.
    #[test]
    fn replacing_the_line_puts_the_cursor_at_its_end() {
        let mut cl = typed("open abc");
        cl.input(key(KeyCode::Enter), Graph::new);
        cl.open();
        cl.input(key(KeyCode::Up), Graph::new);
        assert_eq!(cl.line(), Some("open abc"), "premise: recall replaced it");
        cl.input(plain('!'), Graph::new);
        assert_eq!(
            cl.line(),
            Some("open abc!"),
            "typing appends, not inserts mid-line"
        );
    }

    /// `<C-w>` cuts the word **before the cursor** and keeps what follows.
    ///
    /// All three existing `ctrl_w_*` tests press it with the cursor at the end,
    /// where cutting-back and truncating-from-the-end are the same thing — so
    /// replacing `replace_range(cut..cursor, "")` with `truncate(cut)`, which
    /// eats everything after the cursor, left 1017 tests green. Its sibling
    /// `c_CTRL-U` got its mid-line case; this one did not (Reviewer C).
    #[test]
    fn ctrl_w_keeps_what_is_after_the_cursor() {
        let mut cl = typed("alpha beta gamma");
        cl.input(key(KeyCode::Left), Graph::new);
        cl.input(key(KeyCode::Left), Graph::new);
        cl.input(key(KeyCode::Left), Graph::new);
        cl.input(ctrl('w'), Graph::new);
        assert_eq!(cl.line(), Some("alpha beta mma"));
        cl.input(plain('X'), Graph::new);
        assert_eq!(
            cl.line(),
            Some("alpha beta Xmma"),
            "and the cursor came with it"
        );
    }

    /// **`<C-w>` cuts one word as vim's `c_CTRL-W` does** — a path is many
    /// words (`crate::word`), so `:e ~/naïve/é` loses `é`, then `/`, then
    /// `naïve`; a multibyte word is cut on its char boundaries.
    #[test]
    fn ctrl_w_cuts_a_word_of_a_path() {
        let mut cl = typed("e ~/naïve/é");
        cl.input(ctrl('w'), Graph::new);
        assert_eq!(cl.line(), Some("e ~/naïve/"));
        cl.input(ctrl('w'), Graph::new);
        assert_eq!(cl.line(), Some("e ~/naïve"));
        cl.input(ctrl('w'), Graph::new);
        assert_eq!(cl.line(), Some("e ~/"));
    }

    /// Backspace at column 0 of a **non-empty** line does nothing; on an empty
    /// line it leaves command mode. Nothing enforced the first half, and the
    /// wrong behaviour destroys a typed command: `if buf.is_empty() ||
    /// self.cursor == 0 { self.line = None }` passed 1017 tests (Reviewer C).
    #[test]
    fn backspace_at_the_start_of_a_line_is_not_a_cancel() {
        let mut cl = typed("open abc");
        cl.input(key(KeyCode::Home), Graph::new);
        cl.input(key(KeyCode::Backspace), Graph::new);
        assert_eq!(cl.line(), Some("open abc"), "the line survives");
        cl.input(key(KeyCode::Backspace), Graph::new);
        assert_eq!(cl.line(), Some("open abc"), "…and keeps surviving");

        // Empty: vim leaves.
        let mut empty = typed("");
        empty.input(key(KeyCode::Backspace), Graph::new);
        assert_eq!(empty.line(), None, "backspace past the `:` closes it");
    }

    /// Vim's `c_CTRL-B` / `c_CTRL-E` — start and end of line. Deleting both arms
    /// left 1017 tests green (Reviewer C).
    #[test]
    fn ctrl_b_and_ctrl_e_are_home_and_end() {
        let mut cl = typed("abc");
        cl.input(ctrl('b'), Graph::new);
        cl.input(plain('X'), Graph::new);
        assert_eq!(cl.line(), Some("Xabc"));
        cl.input(ctrl('e'), Graph::new);
        cl.input(plain('Z'), Graph::new);
        assert_eq!(cl.line(), Some("XabcZ"));
    }

    /// `<C-w>` kills the word before the cursor, as vim's `c_CTRL-W` does.
    #[test]
    fn ctrl_w_kills_a_word() {
        let mut cl = typed("open abc def");
        cl.input(ctrl('w'), Graph::new);
        assert_eq!(cl.line(), Some("open abc "));
        cl.input(ctrl('w'), Graph::new);
        assert_eq!(cl.line(), Some("open "));
    }

    /// **The separator is not always one byte.** Option+Space on macOS types a
    /// non-breaking space (U+00A0, two bytes); a byte offset one past where it
    /// *starts* lands mid-character, and `String::truncate` panics on a
    /// non-boundary. Nothing catches the unwind, so this ends the session.
    ///
    /// Every other word-boundary editor here already knows this
    /// (`[[feedback_char_boundary_slicing]]`) — `prompt.rs` works over
    /// `Vec<char>`, `help_view.rs` derives its cut from `len_utf8`. This file
    /// reinvented the operation in raw bytes, and had no tests at all.
    #[test]
    fn ctrl_w_survives_multibyte_whitespace() {
        for sep in ['\u{a0}', '\u{2009}', '\u{3000}'] {
            let mut cl = typed(&format!("foo{sep}bar"));
            cl.input(ctrl('w'), Graph::new);
            assert_eq!(
                cl.line(),
                Some(format!("foo{sep}").as_str()),
                "separator U+{:04X} must be kept whole",
                sep as u32
            );
        }
    }

    /// A word made only of multibyte characters is still a word.
    #[test]
    fn ctrl_w_kills_a_multibyte_word() {
        let mut cl = typed("open 日本語");
        cl.input(ctrl('w'), Graph::new);
        assert_eq!(cl.line(), Some("open "));
    }

    /// `<C-u>` kills the line; a chord with no meaning types nothing.
    #[test]
    fn ctrl_u_kills_the_line_and_unknown_chords_type_nothing() {
        let mut cl = typed("open abc");
        cl.input(ctrl('u'), Graph::new);
        assert_eq!(cl.line(), Some(""));
        // Vim's `c_CTRL-U` kills what is BEFORE the cursor, which is the whole
        // line only when the cursor is at the end. The distinction could not
        // exist while the line had no cursor.
        let mut mid = typed("open abc");
        mid.input(key(KeyCode::Left), Graph::new);
        mid.input(key(KeyCode::Left), Graph::new);
        mid.input(ctrl('u'), Graph::new);
        assert_eq!(mid.line(), Some("bc"));
        cl.input(ctrl('z'), Graph::new);
        assert_eq!(cl.line(), Some(""), "an unbound chord is not its letter");
    }
}
