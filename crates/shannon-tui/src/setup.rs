//! **`shannon setup`** — how Shannon reaches a model: the Anthropic API,
//! Claude Code, or the demo stub.
//!
//! Two screens (Steven, 10-04). The first is the choice, saying plainly what
//! each gives up: the API is fully featured; Claude Code is limited — "you
//! get the TUI features, but you don't have as much control"; the demo
//! connects to no agent at all. The second is what the choice needs: a key
//! for the API, a login for Claude Code. The demo needs nothing.
//!
//! **Every screen can be left, and leaving changes nothing.** Nothing forces a
//! choice: Shannon runs on the stub without one.
//!
//! This module is the state and its drawing. What an answer DOES — saving the
//! key, swapping the backend, asking `claude` whether it is logged in, writing
//! `[tui] model` — is the app's, handed an [`Effect`] (`App::apply_setup`), so
//! everything here is testable without a filesystem, a terminal, or `claude`.

use std::fmt;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use shannon_core::backend::claude_code::LoginStatus;

use crate::style::Ink;
use crate::theme::Theme;

/// The three ways Shannon can answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// The Anthropic API, with a key: every feature.
    Api,
    /// `claude -p`, under the user's own Claude Code login: limited.
    ClaudeCode,
    /// The canned stub: no agent.
    Demo,
}

impl Choice {
    /// In the order the menu lists them, numbered from 1.
    pub const ALL: [Choice; 3] = [Choice::Api, Choice::ClaudeCode, Choice::Demo];

    /// Its row in the menu — from [`ALL`](Self::ALL), so a reordering moves
    /// every place that lands the cursor on it (they were literals).
    fn index(self) -> usize {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }

    fn title(self) -> &'static str {
        match self {
            Choice::Api => "Anthropic API",
            Choice::ClaudeCode => "Claude Code",
            Choice::Demo => "Demo mode",
        }
    }

    /// The one-word verdict, and the role it is said in.
    fn verdict(self) -> Option<(&'static str, Ink)> {
        match self {
            Choice::Api => Some(("Fully featured.", Ink::Safe)),
            Choice::ClaudeCode => Some(("Limited.", Ink::Caution)),
            Choice::Demo => None,
        }
    }

    /// What it means, a line at a time, short enough for the description
    /// column at the narrowest box.
    fn gist(self) -> &'static [&'static str] {
        match self {
            Choice::Api => &[
                "Shannon runs every tool call: you can edit any",
                "call, and output is redacted before the model",
                "sees it. Needs an Anthropic API key.",
            ],
            Choice::ClaudeCode => &[
                "You get the TUI features, but you don't have as",
                "much control: Claude Code runs the tools, and its",
                "model sees their output unredacted. Uses your",
                "`claude` login.",
            ],
            Choice::Demo => &[
                "Lets you try Shannon Code, but doesn't connect to",
                "an agent.",
            ],
        }
    }
}

/// **How setup was entered** — which decides what finishing it changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// From its menu: `shannon setup`, `:setup`, a first launch. The choice is
    /// the default, saved to `[tui] model`, and this session switches to it.
    Menu,
    /// By switching a session to a model that is not set up: make THAT model
    /// work, and change nothing else.
    Model,
    /// To ask whether Claude Code may run in a directory — at launch, after
    /// setup or `:model` brings Claude Code in, or before a prompt would
    /// start it there.
    Trust,
    /// `:models`: the session's model, effort and context budget
    /// (`crate::model_picker`).
    Models,
    /// A bare `:effort`: the session's effort, in a panel at the bottom
    /// (`crate::effort_panel`).
    Effort,
    /// `:memories` / `:skills`: what the model is told beyond the
    /// conversation (`crate::shelf`).
    Shelf,
}

/// **The trust screen's answers** (Steven, 10-04: "yes (remember), one time,
/// and no") — in that order, the safe one last and also `Esc`'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustAnswer {
    /// Trust it from now on: kept.
    Remember,
    /// Trust it until Shannon quits.
    ThisRun,
    /// Do not: Claude Code does not run here.
    No,
}

impl TrustAnswer {
    pub const ALL: [TrustAnswer; 3] =
        [TrustAnswer::Remember, TrustAnswer::ThisRun, TrustAnswer::No];

    fn title(self) -> &'static str {
        match self {
            TrustAnswer::Remember => "Yes, and remember",
            TrustAnswer::ThisRun => "Yes, this time only",
            TrustAnswer::No => "No",
        }
    }

    fn gist(self) -> &'static str {
        match self {
            TrustAnswer::Remember => "here and the folders under it, from now on",
            TrustAnswer::ThisRun => "until Shannon quits; the next launch asks again",
            TrustAnswer::No => "Claude Code does not run here; sending asks again",
        }
    }
}

/// **An API key being typed or pasted** — never drawn, never logged, never
/// printed by `{:?}` (which says only how long it is).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Key(String);

impl Key {
    /// A key from wherever it was found — launch's lookup (`keys::find_key`).
    pub fn new(text: impl Into<String>) -> Key {
        Key(text.into())
    }

    /// The key, for the places it goes: the API client, and the file it is
    /// saved to.
    pub fn expose(self) -> String {
        self.0
    }

    fn chars(&self) -> usize {
        self.0.chars().count()
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({} chars)", self.chars())
    }
}

/// What the screens state, gathered by the app when setup opens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    /// Where a key is saved, as the user should read it — `None` when there
    /// is nowhere (no `HOME`, no `XDG_CONFIG_HOME`).
    pub key_path: Option<String>,
    /// A variable that is read ahead of a saved key, if one is set — the name,
    /// never its value.
    pub key_overridden_by: Option<&'static str>,
    /// The API answers already, with a key from somewhere.
    pub has_key: bool,
    /// Where the key in use came from, as the user should read it, when
    /// Shannon holds it — what Save writes when nothing is pasted.
    pub key_from: Option<String>,
    /// The key in use is the saved one: saving it again would change nothing.
    pub key_is_saved: bool,
}

/// **What the key screen can do** with the key — pasted, or else the one in
/// use (Steven, 10-04: "use", "save", "exit").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyAction {
    /// Answer from it now; nothing is written.
    Use,
    /// Write it, owner-only, and answer from it.
    Save,
    /// Leave setup; nothing changes.
    Exit,
}

impl KeyAction {
    const ALL: [KeyAction; 3] = [KeyAction::Use, KeyAction::Save, KeyAction::Exit];

    fn index(self) -> usize {
        Self::ALL.iter().position(|a| *a == self).unwrap_or(0)
    }
}

/// The screen showing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Screen {
    Choose {
        at: usize,
    },
    Api {
        key: Key,
        said: Option<String>,
        /// The action the cursor is on, in [`KeyAction::ALL`].
        at: usize,
    },
    ClaudeCode {
        status: Option<LoginStatus>,
        said: Option<String>,
    },
    Trust {
        dir: PathBuf,
        /// `dir` as the user reads it (`~/…`) — the app's to spell.
        shown: String,
        /// A prompt waits on the answer.
        held: bool,
        /// What the folder's `.shannon/` holds (`trust::project_held`),
        /// which a yes applies — named on the screen,
        /// the one place a user at it can see them (Steven, 10-13: the
        /// "not read" note never showed).
        holds: Vec<String>,
        /// The answer the cursor is on, in [`TrustAnswer::ALL`].
        at: usize,
    },
    Models(Box<crate::model_picker::Picker>),
    Effort(Box<crate::effort_panel::Panel>),
    Shelf(Box<crate::shelf::Screen>),
}

/// **What the app must do about a key** — the only way setup reaches
/// anything outside itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Nothing; the screen changed, or did not.
    None,
    /// Close setup. Nothing was chosen.
    Leave,
    /// Ask `claude` whether it is logged in, then [`Setup::set_status`].
    CheckClaudeCode,
    /// Answer from this key — `None`: the one in use — writing nothing; then
    /// [`Effect::Finish`] with the API.
    UseKey(Option<Key>),
    /// Save this key — `None`: the one in use — and answer from it; on
    /// success, [`Effect::Finish`] with the API.
    SaveKey(Option<Key>),
    /// The choice is made and ready.
    Finish(Choice),
    /// The trust screen answered for `dir`; `held`: a prompt waits on it.
    Trust {
        dir: PathBuf,
        answer: TrustAnswer,
        held: bool,
    },
    /// `:models` was applied to `episode`, the session it opened on: each
    /// field as chosen.
    Models {
        episode: shannon_core::EpisodeId,
        chosen: crate::model_picker::Chosen,
    },
    /// The `:effort` panel ran a line for `episode`, the session it opened
    /// on: its effort (`None`: follow the run's).
    Effort {
        episode: shannon_core::EpisodeId,
        effort: Option<shannon_core::Effort>,
    },
    /// `:memories` / `:skills`: edit this file in `$EDITOR`, the screen
    /// staying open.
    Edit(PathBuf),
    /// `:memories` / `:skills`: disable a file or an MCP server, or enable
    /// it again, for the session — the screen staying open.
    Toggle(crate::shelf::Toggle),
    /// `:memories` / `:skills`: remove a file — ask what goes, or, confirmed,
    /// remove it — the screen staying open.
    Remove(crate::shelf::Removal),
    /// `:memories` / `:skills`: a new one from a template, in `$EDITOR`.
    Create(crate::shelf::Create),
    /// `:memories` / `:skills`: leave, a request for the model to write one
    /// drafted in the prompt.
    AskModel(crate::shelf::Shelf),
    /// `:memories` / `:skills`: a key its list does not claim — navigation,
    /// which the shared keymap answers (`App::navigate_shelf`).
    Navigate(crossterm::event::KeyEvent),
}

/// **The cursor follows the key field's first and last character**: a key
/// arriving moves it to Save — what a first launch's paste is for — and the
/// field emptied moves it back to Use, the key in use. Anywhere between, it
/// stays where the user put it.
fn follow_the_key(was_empty: bool, typed: &str, at: &mut usize) {
    match (was_empty, typed.is_empty()) {
        (true, false) => *at = KeyAction::Save.index(),
        (false, true) => *at = KeyAction::Use.index(),
        _ => {}
    }
}

/// **Where a screen is drawn.** Every one takes every key while open; what
/// differs is what stays readable behind it — drawn with no focus, so
/// nothing there looks like it takes keys (`view::draw`).
pub enum Placement<'a> {
    /// Over the whole frame: setup, the trust question, `:models`.
    Whole,
    /// At the bottom, the panes above it: `:effort` ([`draw_panel`]).
    Panel(&'a crate::effort_panel::Panel),
    /// A float over the panes, its keys on the status row:
    /// `:memories` / `:skills` ([`draw_float`]).
    Float(&'a crate::shelf::Screen),
}

/// `shannon setup`, open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setup {
    screen: Screen,
    entry: Entry,
    facts: Facts,
    /// Why setup opened on its own, for one entered by a model switch —
    /// said at the top of the screen it opened on.
    why: Option<String>,
}

impl Setup {
    /// The menu: the choice of how Shannon answers.
    pub fn menu(facts: Facts) -> Setup {
        Setup {
            screen: Screen::Choose { at: 0 },
            entry: Entry::Menu,
            facts,
            why: None,
        }
    }

    /// **Straight to what `choice` needs**, because a session was switched to
    /// a model that needs it — `why` says which. Leaving goes back to the
    /// session, not to the menu: nobody asked for the menu.
    pub fn at(choice: Choice, why: String, facts: Facts, status: Option<LoginStatus>) -> Setup {
        let screen = match choice {
            Choice::Api => Screen::Api {
                key: Key::default(),
                said: None,
                at: KeyAction::Use.index(),
            },
            Choice::ClaudeCode => Screen::ClaudeCode { status, said: None },
            // No caller asks for this (`App::offer_setup_for` opens the two
            // that need setting up); were one to, the menu is all Demo has.
            Choice::Demo => Screen::Choose {
                at: Choice::Demo.index(),
            },
        };
        Setup {
            screen,
            entry: Entry::Model,
            facts,
            why: Some(why),
        }
    }

    /// **Whether Claude Code may run in `dir`** — Claude Code's and Copilot's
    /// question, asked before anything runs there rather than as a y/n on
    /// the status line after the first message (Steven, 10-04). `shown` is
    /// `dir` as the user reads it; `held`: a prompt waits on the answer;
    /// `holds`: Shannon's own settings there, which a yes applies.
    pub fn trust(dir: PathBuf, shown: String, held: bool, holds: Vec<String>) -> Setup {
        Setup {
            screen: Screen::Trust {
                dir,
                shown,
                held,
                holds,
                at: 0,
            },
            entry: Entry::Trust,
            facts: Facts::default(),
            why: None,
        }
    }

    /// **`:models`**, open on `picker` — no setup to do, so no facts.
    pub fn models(picker: crate::model_picker::Picker) -> Setup {
        Setup {
            screen: Screen::Models(Box::new(picker)),
            entry: Entry::Models,
            facts: Facts::default(),
            why: None,
        }
    }

    /// **A bare `:effort`**, open on `panel`.
    pub fn effort(panel: crate::effort_panel::Panel) -> Setup {
        Setup {
            screen: Screen::Effort(Box::new(panel)),
            entry: Entry::Effort,
            facts: Facts::default(),
            why: None,
        }
    }

    /// **`:memories` / `:skills`**, open on `screen`.
    pub fn shelf(screen: crate::shelf::Screen) -> Setup {
        Setup {
            screen: Screen::Shelf(Box::new(screen)),
            entry: Entry::Shelf,
            facts: Facts::default(),
            why: None,
        }
    }

    /// **The screen as plain text**, `width` by `rows`, its legend last —
    /// what a test reads without a terminal.
    #[cfg(test)]
    pub(crate) fn shown(&self, width: usize, rows: usize) -> String {
        let (lines, legend) = self.page(width, rows, &Theme::terminal());
        lines
            .iter()
            .chain(std::iter::once(&legend))
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The shelf screen, when that is what is open — for the app to read
    /// its lists again (`App::refresh_shelves`).
    pub fn shelf_mut(&mut self) -> Option<&mut crate::shelf::Screen> {
        match &mut self.screen {
            Screen::Shelf(screen) => Some(screen),
            _ => None,
        }
    }

    /// The file under the open shelf's cursor.
    #[cfg(test)]
    pub fn shelf_selected(&self) -> Option<PathBuf> {
        match &self.screen {
            Screen::Shelf(screen) => screen.selected().map(|row| row.path.clone()),
            _ => None,
        }
    }

    /// The `:memories` / `:skills` screen, open — for a test to draw.
    #[cfg(test)]
    pub fn shelf_screen(&self) -> Option<&crate::shelf::Screen> {
        match &self.screen {
            Screen::Shelf(screen) => Some(screen),
            _ => None,
        }
    }

    /// **Where what is open is drawn** — the one answer `view::draw` asks: a
    /// panel at the bottom ([`draw_panel`]), a float ([`draw_float`]), or the
    /// whole frame ([`draw`]).
    pub fn placement(&self) -> Placement<'_> {
        match &self.screen {
            Screen::Effort(panel) => Placement::Panel(panel),
            Screen::Shelf(screen) => Placement::Float(screen),
            _ => Placement::Whole,
        }
    }

    pub fn entry(&self) -> Entry {
        self.entry
    }

    /// The directory the trust screen asks about, if that is what is open.
    #[cfg(test)]
    pub(crate) fn trust_asked(&self) -> Option<&std::path::Path> {
        match &self.screen {
            Screen::Trust { dir, .. } => Some(dir),
            _ => None,
        }
    }

    /// The trust screen's answer `answer`, as the effect the app acts on.
    fn answer_trust(&self, answer: TrustAnswer) -> Effect {
        match &self.screen {
            Screen::Trust { dir, held, .. } => Effect::Trust {
                dir: dir.clone(),
                answer,
                held: *held,
            },
            _ => Effect::None,
        }
    }

    /// The answer to [`Effect::CheckClaudeCode`].
    pub fn set_status(&mut self, status: LoginStatus) {
        if let Screen::ClaudeCode { status: slot, said } = &mut self.screen {
            *slot = Some(status);
            *said = None;
        }
    }

    /// Something the app has to say on the open screen — a key that could
    /// not be saved, say. Kept on the screen, where the user is looking.
    pub fn say(&mut self, what: String) {
        match &mut self.screen {
            Screen::Api { said, .. } | Screen::ClaudeCode { said, .. } => *said = Some(what),
            Screen::Choose { .. }
            | Screen::Trust { .. }
            | Screen::Models(_)
            | Screen::Effort(_)
            | Screen::Shelf(_) => {}
        }
    }

    /// One key. `Ctrl-C` leaves from anywhere; `Esc` goes back a screen, or
    /// leaves from the first one (and from one a model switch opened).
    pub fn key(&mut self, key: KeyEvent) -> Effect {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            // Leaving the trust question is answering it, safely.
            return match self.screen {
                Screen::Trust { .. } => self.answer_trust(TrustAnswer::No),
                _ => Effect::Leave,
            };
        }
        let back = |this: &mut Setup, from: usize| match this.entry {
            Entry::Menu => {
                this.screen = Screen::Choose { at: from };
                Effect::None
            }
            Entry::Model | Entry::Trust | Entry::Models | Entry::Effort | Entry::Shelf => {
                Effect::Leave
            }
        };
        match &mut self.screen {
            Screen::Models(picker) => picker.key(key),
            Screen::Effort(panel) => panel.key(key),
            Screen::Shelf(screen) => screen.key(key),
            // The reflexive keys are the safe answer, as on the decision pane.
            Screen::Trust { at, .. } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.answer_trust(TrustAnswer::No),
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                    *at = (*at + 1).min(TrustAnswer::ALL.len() - 1);
                    Effect::None
                }
                KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                    *at = at.saturating_sub(1);
                    Effect::None
                }
                KeyCode::Enter => {
                    let answer = TrustAnswer::ALL[*at];
                    self.answer_trust(answer)
                }
                KeyCode::Char(c) => match c.to_digit(10).map(|d| d as usize) {
                    Some(n) if (1..=TrustAnswer::ALL.len()).contains(&n) => {
                        self.answer_trust(TrustAnswer::ALL[n - 1])
                    }
                    _ => Effect::None,
                },
                _ => Effect::None,
            },
            Screen::Choose { at } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => Effect::Leave,
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                    *at = (*at + 1).min(Choice::ALL.len() - 1);
                    Effect::None
                }
                KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                    *at = at.saturating_sub(1);
                    Effect::None
                }
                KeyCode::Enter => {
                    let choice = Choice::ALL[*at];
                    self.choose(choice)
                }
                KeyCode::Char(c) => match c.to_digit(10).map(|d| d as usize) {
                    Some(n) if (1..=Choice::ALL.len()).contains(&n) => {
                        self.choose(Choice::ALL[n - 1])
                    }
                    _ => Effect::None,
                },
                _ => Effect::None,
            },
            Screen::Api {
                key: typed,
                said,
                at,
            } => {
                // Letters type the key, so the actions are chosen by cursor.
                let was_empty = typed.0.is_empty();
                let effect = match key.code {
                    KeyCode::Esc => return back(self, Choice::Api.index()),
                    KeyCode::Down | KeyCode::Tab => {
                        *at = (*at + 1).min(KeyAction::ALL.len() - 1);
                        Effect::None
                    }
                    KeyCode::Up | KeyCode::BackTab => {
                        *at = at.saturating_sub(1);
                        Effect::None
                    }
                    KeyCode::Enter => {
                        let pasted =
                            Some(Key(typed.0.trim().to_string())).filter(|k| !k.0.is_empty());
                        match (KeyAction::ALL[*at], pasted) {
                            (KeyAction::Exit, _) => Effect::Leave,
                            (KeyAction::Use, pasted @ Some(_)) => Effect::UseKey(pasted),
                            (KeyAction::Save, pasted @ Some(_)) => Effect::SaveKey(pasted),
                            // Nothing pasted: the key in use, if there is one.
                            (KeyAction::Use, None) if self.facts.has_key => Effect::UseKey(None),
                            (KeyAction::Save, None) if self.facts.key_from.is_some() => {
                                Effect::SaveKey(None)
                            }
                            (KeyAction::Use | KeyAction::Save, None) => {
                                *said = Some("paste a key first — or Esc to go back".into());
                                Effect::None
                            }
                        }
                    }
                    KeyCode::Char('u') if ctrl => {
                        typed.0.clear();
                        Effect::None
                    }
                    KeyCode::Backspace => {
                        typed.0.pop();
                        Effect::None
                    }
                    KeyCode::Char(c) if !ctrl && !c.is_whitespace() => {
                        typed.0.push(c);
                        *said = None;
                        Effect::None
                    }
                    _ => Effect::None,
                };
                follow_the_key(was_empty, &typed.0, at);
                effect
            }
            Screen::ClaudeCode { status, said } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => back(self, Choice::ClaudeCode.index()),
                KeyCode::Char('r') => Effect::CheckClaudeCode,
                // Ready, or unknown — "could not tell" is not "no", and the
                // first turn says what is wrong if anything is.
                KeyCode::Enter => match status {
                    Some(LoginStatus::LoggedIn(_) | LoginStatus::Unknown(_)) => {
                        Effect::Finish(Choice::ClaudeCode)
                    }
                    _ => {
                        *said = Some("not ready yet — set it up as above, then r".into());
                        Effect::None
                    }
                },
                _ => Effect::None,
            },
        }
    }

    /// A paste: on the key screen it IS the key (replacing what was typed —
    /// a second paste must not double it); anywhere else it is nothing.
    pub fn paste(&mut self, text: &str) -> Effect {
        if let Screen::Api { key, said, at } = &mut self.screen {
            let was_empty = key.0.is_empty();
            key.0 = text.split_whitespace().collect();
            *said = None;
            follow_the_key(was_empty, &key.0, at);
        }
        Effect::None
    }

    fn choose(&mut self, choice: Choice) -> Effect {
        match choice {
            Choice::Api => {
                self.screen = Screen::Api {
                    key: Key::default(),
                    said: None,
                    at: KeyAction::Use.index(),
                };
                Effect::None
            }
            Choice::ClaudeCode => {
                self.screen = Screen::ClaudeCode {
                    status: None,
                    said: None,
                };
                Effect::CheckClaudeCode
            }
            Choice::Demo => Effect::Finish(Choice::Demo),
        }
    }

    /// **One action on the key screen, saying what it would do** — with the
    /// key pasted, or else the one in use; dimmed where it would do nothing
    /// but say so.
    fn key_action_line(
        &self,
        action: KeyAction,
        here: bool,
        pasted: bool,
        theme: &Theme,
    ) -> Line<'static> {
        let saved_to = self.facts.key_path.as_deref();
        let (name, what, open) = match action {
            KeyAction::Use if pasted => (
                "Use",
                "answer from this key now; nothing is written".into(),
                true,
            ),
            KeyAction::Use if self.facts.has_key => {
                ("Use", "keep answering from the key in use".into(), true)
            }
            KeyAction::Use => ("Use", "paste a key first".into(), false),
            KeyAction::Save => match (saved_to, pasted) {
                (None, _) => (
                    "Save",
                    "nowhere to save it: set HOME or XDG_CONFIG_HOME".into(),
                    false,
                ),
                (Some(path), true) => (
                    "Save",
                    format!("write this key to {path}, readable only by you"),
                    true,
                ),
                (Some(_), false) if self.facts.key_is_saved => {
                    ("Save", "the key in use is saved there already".into(), true)
                }
                (Some(path), false) if self.facts.key_from.is_some() => (
                    "Save",
                    format!("write the key in use to {path}, readable only by you"),
                    true,
                ),
                (Some(_), false) => ("Save", "paste a key first".into(), false),
            },
            KeyAction::Exit => ("Exit", "leave setup; nothing changes".into(), true),
        };
        let head = chosen_head(here, theme);
        let body = if open {
            Style::default()
        } else {
            theme.ui(Ink::TextQuiet)
        };
        Line::from(vec![
            Span::styled(format!("{} {name:<6}", if here { "▸" } else { " " }), head),
            Span::styled(what, body),
        ])
    }

    /// The screen as lines, and its legend — split from [`draw`] so a test
    /// reads what is said without a terminal. `width` and `rows` are the
    /// box's room, for a screen that scrolls (`crate::shelf`).
    fn page(
        &self,
        width: usize,
        rows: usize,
        theme: &Theme,
    ) -> (Vec<Line<'static>>, Line<'static>) {
        let dim = theme.ui(Ink::TextQuiet);
        let mut out: Vec<Line<'static>> = Vec::new();
        if let Some(why) = &self.why {
            out.push(Line::from(Span::styled(why.clone(), dim)));
            out.push(Line::from(""));
        }
        let legend = match &self.screen {
            Screen::Choose { at } => {
                out.push(Line::from("How should Shannon reach a model?"));
                for (i, choice) in Choice::ALL.into_iter().enumerate() {
                    out.push(Line::from(""));
                    let here = i == *at;
                    let mark = if here { "▸" } else { " " };
                    let head = chosen_head(here, theme);
                    let mut first = vec![
                        Span::styled(format!("{mark} {}  ", i + 1), head),
                        Span::styled(format!("{:<15}", choice.title()), head),
                    ];
                    match choice.verdict() {
                        Some((word, role)) => {
                            first.push(Span::styled(word, theme.ui(role)));
                            out.push(Line::from(first));
                        }
                        None => {
                            first.push(Span::raw(choice.gist()[0]));
                            out.push(Line::from(first));
                        }
                    }
                    let rest = match choice.verdict() {
                        Some(_) => choice.gist(),
                        None => &choice.gist()[1..],
                    };
                    for line in rest {
                        out.push(Line::from(format!("{:20}{line}", "")));
                    }
                }
                out.push(Line::from(""));
                out.push(Line::from(Span::styled(
                    "Use :setup to return to this screen at any time.",
                    dim,
                )));
                // The "1–3" below is the length of `Choice::ALL`, spelled out.
                const _: () = assert!(Choice::ALL.len() == 3, "the legend says 1–3");
                "j/k move · Enter or 1–3 choose · Esc leave"
            }
            Screen::Api { key, said, at } => {
                out.push(Line::from(Span::styled(
                    "Anthropic API",
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                out.push(Line::from(""));
                out.push(Line::from(
                    "Paste your API key (console.anthropic.com → API Keys).",
                ));
                out.push(Line::from(""));
                let shown = match key.chars() {
                    0 => Span::styled("(nothing yet)", dim),
                    n => Span::raw(format!("{}  ({n} characters)", "●".repeat(n.min(24)))),
                };
                out.push(Line::from(vec![Span::styled("  key  ", dim), shown]));
                if let Some(from) = self.facts.key_from.as_ref().filter(|_| key.chars() == 0) {
                    out.push(Line::from(Span::styled(
                        format!("  in use: the key from {from}"),
                        dim,
                    )));
                }
                out.push(Line::from(""));
                for (i, action) in KeyAction::ALL.into_iter().enumerate() {
                    out.push(self.key_action_line(action, i == *at, key.chars() > 0, theme));
                }
                out.push(Line::from(""));
                let caution = theme.ui(Ink::Caution);
                if key.chars() > 0 && !key.0.starts_with("sk-ant-") {
                    out.push(Line::from(Span::styled(
                        "This doesn't look like an Anthropic key (sk-ant-…) — it is used anyway.",
                        caution,
                    )));
                }
                if let Some(var) = self.facts.key_overridden_by {
                    out.push(Line::from(Span::styled(
                        format!("{var} is set: at launch it is used ahead of a saved key."),
                        caution,
                    )));
                }
                if let Some(said) = said {
                    out.push(Line::from(Span::styled(
                        said.clone(),
                        theme.ui(Ink::Danger),
                    )));
                }
                "↑/↓ move · Enter choose · Ctrl-U clear · Esc back"
            }
            Screen::ClaudeCode { status, said } => {
                out.push(Line::from(Span::styled(
                    "Claude Code",
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                out.push(Line::from(""));
                let (mark, role, text) = match status {
                    Some(LoginStatus::LoggedIn(how)) => {
                        ("✓", Ink::Safe, format!("Logged in ({how})."))
                    }
                    Some(LoginStatus::NotLoggedIn) => (
                        "✗",
                        Ink::Danger,
                        "Not logged in — run `claude` in a terminal and log in, then press r."
                            .to_string(),
                    ),
                    Some(LoginStatus::NotInstalled) => (
                        "✗",
                        Ink::Danger,
                        "`claude` is not installed — install Claude Code, log in with \
                         `claude`, then press r."
                            .to_string(),
                    ),
                    Some(LoginStatus::Unknown(why)) => (
                        "?",
                        Ink::Caution,
                        format!("Could not tell: {why}. Enter uses it anyway; r checks again."),
                    ),
                    None => (
                        "…",
                        Ink::TextQuiet,
                        "Not checked yet — r checks.".to_string(),
                    ),
                };
                out.push(Line::from(vec![
                    Span::styled(format!("{mark} "), theme.ui(role)),
                    Span::raw(text),
                ]));
                out.push(Line::from(""));
                for line in [
                    "Shannon decides every tool call Claude Code makes, and stops a",
                    "turn if one runs without asking. Credential paths are refused",
                    "by default ([claude_code] path_policy).",
                    "It reads your CLAUDE.md files and runs a project's own Claude",
                    "Code settings, so Shannon asks before it runs in a directory.",
                    "A turn is slower than on the API.",
                ] {
                    out.push(Line::from(Span::styled(line, dim)));
                }
                // **Claude Code's limit, said where you choose it** (Steven,
                // 10-22: "let's make sure that in setup we mention the timeout
                // limitation"): its hook waits an hour at most, and refuses
                // first. The API waits as long as you do. Both numbers from
                // their constants: "an hour" was a literal for one beside the
                // other's derived minutes (review stop 8, A 10).
                use shannon_core::backend::claude_code::{HOOK_DEADLINE, HOOK_TIMEOUT};
                let (minutes, most) = (HOOK_DEADLINE.as_secs() / 60, HOOK_TIMEOUT.as_secs() / 60);
                for line in [
                    format!("A call left undecided for {minutes} minutes is refused:"),
                    format!("Claude Code waits at most {most} minutes for an answer."),
                ] {
                    out.push(Line::from(Span::styled(line, dim)));
                }
                if let Some(said) = said {
                    out.push(Line::from(""));
                    out.push(Line::from(Span::styled(
                        said.clone(),
                        theme.ui(Ink::Danger),
                    )));
                }
                match status {
                    Some(LoginStatus::LoggedIn(_) | LoginStatus::Unknown(_)) => {
                        "Enter continue · r check again · Esc back"
                    }
                    _ => "r check again · Esc back",
                }
            }
            Screen::Trust {
                shown, at, holds, ..
            } => {
                out.push(Line::from(Span::styled(
                    "Do you trust the files in this folder?",
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                out.push(Line::from(""));
                out.push(Line::from(format!("  {shown}")));
                out.push(Line::from(""));
                for line in [
                    "Claude Code runs a project's own settings here — its hooks and its",
                    "MCP servers — and reads its CLAUDE.md files; Shannon applies what its",
                    ".shannon/ holds. Trust a folder you would run `claude` in yourself.",
                ] {
                    out.push(Line::from(Span::styled(line, dim)));
                }
                // Not dimmed: what THIS folder holds, which a yes applies.
                if !holds.is_empty() {
                    out.push(Line::from(""));
                    out.push(Line::from(
                        "This folder holds Shannon settings, which trusting it applies:",
                    ));
                    for file in holds {
                        out.push(Line::from(format!("  {file}")));
                    }
                }
                out.push(Line::from(""));
                for (i, answer) in TrustAnswer::ALL.into_iter().enumerate() {
                    let here = i == *at;
                    let head = chosen_head(here, theme);
                    out.push(Line::from(vec![
                        Span::styled(
                            format!("{} {}  ", if here { "▸" } else { " " }, i + 1),
                            head,
                        ),
                        Span::styled(format!("{:<21}", answer.title()), head),
                        Span::styled(answer.gist(), dim),
                    ]));
                }
                const _: () = assert!(TrustAnswer::ALL.len() == 3, "the legend says 1–3");
                "j/k move · Enter or 1–3 choose · Esc no"
            }
            Screen::Models(picker) => picker.page(&mut out, theme),
            Screen::Shelf(screen) => screen.page(&mut out, width, rows, theme),
            // Drawn as a panel (`draw_panel`), never as a page; its lines
            // here keep a full-screen draw honest if one ever happens.
            Screen::Effort(panel) => {
                out.extend(panel.lines(theme));
                ""
            }
        };
        // Its keys drawn as keys, as every row of keys is (`paint::hint`).
        let words = crate::style::CellStyle::ink(Ink::Tool);
        let legend = crate::paint::PageLine {
            runs: crate::paint::hint(legend, words),
            style: Style::default(),
        };
        let paint = crate::paint::Painter::new(theme, crate::theme::Context::Code);
        (out, paint.line(&legend))
    }
}

/// **Clear `area` to the theme's ground** — `Clear` resets cells to the
/// terminal's own, so a screen over a themed frame would draw on the
/// terminal's background; the ground goes back on (`view::ground`). Every
/// clear here walks this.
fn clear(frame: &mut Frame, area: Rect, theme: &Theme) {
    frame.render_widget(Clear, area);
    crate::view::ground(frame, area, theme);
}

/// A choice's head: `ui.chosen` under the cursor, bold elsewhere — one
/// statement for the setup screens' three lists.
fn chosen_head(here: bool, theme: &Theme) -> Style {
    if here {
        theme.ui(Ink::Chosen)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

/// **Draw setup over the whole frame.** A box in the middle, wide enough for
/// the menu's description column, with its legend pinned to the last row.
///
/// **What does not fit is said, never silently cut** (Steven, 10-04: in a
/// short pane the menu lost its `:setup` line, a narrow one cuts lines at the
/// right). Lines are folded to the box's width; when the rows do not fit, the
/// blank lines go first; past that, the last row says more is below. While a
/// decision is `waiting` behind it, the legend's row says so, as a panel's
/// bar does ([`draw_panel`]).
pub fn draw(frame: &mut Frame, setup: &Setup, waiting: bool, theme: &Theme) {
    let area = frame.area();
    clear(frame, area, theme);
    let width = area.width.min(76);
    // `fitted`'s room: the box's inner width, its height less borders and
    // the legend.
    let (lines, legend) = setup.page(
        width.saturating_sub(2) as usize,
        (area.height as usize).saturating_sub(3),
        theme,
    );
    let rows = fitted(&lines, width.saturating_sub(2) as usize, area.height, theme);
    // Borders, the legend, and a spacer row above it while there is room.
    let height = (rows.len() as u16 + 4).min(area.height);
    let boxed = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let title = match setup.entry {
        Entry::Trust => " Shannon Code — trust ",
        Entry::Models => " Shannon Code — models ",
        Entry::Effort => " Shannon Code — effort ",
        Entry::Shelf => " Shannon Code — memories and skills ",
        Entry::Menu | Entry::Model => " Shannon Code — setup ",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.ui(Ink::BorderFocused))
        .title(title);
    let inner = block.inner(boxed);
    frame.render_widget(block, boxed);
    if inner.height == 0 {
        return;
    }
    let body = Rect {
        height: inner.height.saturating_sub(1),
        ..inner
    };
    frame.render_widget(Paragraph::new(rows), body);
    let foot = Rect {
        y: inner.y + inner.height - 1,
        height: 1,
        ..inner
    };
    // A decision waiting behind the screen takes the legend's row, as it
    // takes a panel's bar: the keys the legend offers are not the answer.
    let legend = match waiting {
        true => Line::from(" a decision is waiting — Esc leaves this screen, then answer it ")
            .style(theme.ui(Ink::BarWaiting)),
        false => legend,
    };
    // A bar across its row, as the panel's and the float's are: drawn as
    // a styled span, it stopped where its words did (stop 17, V2b L5).
    draw_bar(frame, legend, foot, theme);
}

/// **Draw a panel** — vim's command-line window: its lines the full width
/// just above the status row `status`, under a rule, the panes still drawn
/// above it, and its bar ON the status row. The status row's own account — the focused
/// pane's mode and keys, `^C:stop`, a decision's `a`/`d` — would offer keys
/// the panel takes (Reviewers, 10-04); the bar says the panel's instead, and
/// when a decision is `waiting` behind it, says that.
///
/// The bar is always on screen; when the rows are too few for every line,
/// the lines shown are the ones around the cursor.
pub fn draw_panel(
    frame: &mut Frame,
    panel: &crate::effort_panel::Panel,
    status: Rect,
    waiting: bool,
    theme: &Theme,
) {
    let mut lines = panel.lines(theme);
    let mut bar = lines.pop().unwrap_or_default();
    if waiting {
        bar = Line::styled(
            " a decision is waiting — q closes this panel, then answer it ",
            theme.ui(Ink::BarWaiting),
        );
    }
    draw_bar(frame, bar, status, theme);
    let room = status.y.saturating_sub(frame.area().y) as usize;
    // **A rule above the lines**, as vim's status line stands above `:ls`:
    // the lines are drawn over the panes, and each box they cover ended
    // open (Steven, 11-09, image 48: "a separator will make the panel look
    // cleaner"). Only with a row to spare beside one line.
    let ruled = room >= 2;
    let shown = room.saturating_sub(usize::from(ruled)).min(lines.len());
    if shown == 0 {
        return;
    }
    let first = (panel.cursor() + 1)
        .saturating_sub(shown)
        .min(lines.len() - shown);
    let rows: Vec<Line<'static>> = lines.drain(first..first + shown).collect();
    let height = rows.len() as u16;
    let area = Rect {
        x: status.x,
        y: status.y - height,
        width: status.width,
        height,
    };
    clear(frame, area, theme);
    frame.render_widget(Paragraph::new(rows), area);
    if ruled {
        let above = Rect {
            y: area.y - 1,
            height: 1,
            ..area
        };
        clear(frame, above, theme);
        let rule = "─".repeat(usize::from(above.width));
        let look = theme.ui(Ink::BorderFocused);
        frame.render_widget(Paragraph::new(Line::styled(rule, look)), above);
    }
}

/// **Draw a float** — `:memories` / `:skills` over the panes (Steven, 10-07:
/// "like a balloon in vim", fzf's `:GFiles`): most of the frame above the
/// status row, centred; the list at the float's own width (image 91: cut at
/// a fixed box), and the file under the cursor beside it when there is room
/// ([`Screen::preview_lines`](crate::shelf::Screen::preview_lines)). Its keys
/// go on the status row `status`, as a panel's bar does — the row's own
/// account would offer the keys of a pane that is not taking them.
pub fn draw_float(
    frame: &mut Frame,
    screen: &crate::shelf::Screen,
    status: Rect,
    waiting: bool,
    theme: &Theme,
) {
    let frame_area = frame.area();
    let above = Rect {
        height: status.y.saturating_sub(frame_area.y),
        ..frame_area
    };
    let boxed = float_rect(above);
    clear(frame, boxed, theme);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.ui(Ink::BorderFocused))
        .title(" memories and skills ");
    let inner = block.inner(boxed);
    frame.render_widget(block, boxed);
    let split = inner.width >= PREVIEW_FROM;
    let list_width = match split {
        true => inner.width * 11 / 20,
        false => inner.width,
    };
    let mut lines = Vec::new();
    let legend = screen.page(
        &mut lines,
        list_width as usize,
        inner.height as usize,
        theme,
    );
    // `fitted` keeps three rows for a box's borders and legend; this one's
    // are outside `inner`.
    let rows = fitted(
        &lines,
        list_width as usize,
        inner.height.saturating_add(3),
        theme,
    );
    frame.render_widget(
        Paragraph::new(rows),
        Rect {
            width: list_width,
            ..inner
        },
    );
    if split {
        let rule = Rect {
            x: inner.x + list_width,
            width: 1,
            ..inner
        };
        let bar =
            vec![Line::from(Span::styled("│", theme.ui(Ink::TextFaint))); inner.height as usize];
        frame.render_widget(Paragraph::new(bar), rule);
        let preview = Rect {
            x: rule.x + 2,
            width: inner.width.saturating_sub(list_width + 2),
            ..inner
        };
        let shown = screen.preview_lines(preview.width as usize, preview.height as usize, theme);
        frame.render_widget(Paragraph::new(shown), preview);
    }
    let bar = match waiting {
        true => Line::styled(
            " a decision is waiting — q leaves this screen, then answer it ",
            theme.ui(Ink::BarWaiting),
        ),
        false => bar_of_keys(legend, " ", theme),
    };
    draw_bar(frame, bar, status, theme);
}

/// **A bar on the status row, its ground across the whole row** — each bar
/// carries its ground as its `Line`'s style, and a line's style reaches only
/// its own characters: the row past them stayed the screen's, and the bar
/// stopped where its words did (Steven, 11-09, image 47: "a little broken").
fn draw_bar(frame: &mut Frame, bar: Line<'static>, row: Rect, theme: &Theme) {
    clear(frame, row, theme);
    let ground = bar.style;
    frame.render_widget(Paragraph::new(bar).style(ground), row);
}

/// **A screen's bar of keys** — ` Enter edit · x disable ` in `ui.bar`,
/// its keys found as every row's are (`paint::hint`) and **underlined in
/// the bar's own colours**. `ui.help.code` alone cut a hole in the fill at
/// each key, and over the fill it cannot be read: under 3:1 against the bar
/// in all 21 shipped themes, 1.3 at the median (review stop 17, A M3).
/// `tail` closes the bar, in its words.
pub(crate) fn bar_of_keys(legend: &str, tail: &str, theme: &Theme) -> Line<'static> {
    use crate::paint::Part;
    let bar = theme.ui(Ink::Bar);
    let key = bar.add_modifier(ratatui::style::Modifier::UNDERLINED);
    let mut spans = vec![Span::styled(" ", bar)];
    spans.extend(
        (crate::paint::key_row(legend).into_iter()).map(|(part, text)| {
            let look = match part {
                Part::Key => key,
                Part::Words => bar,
            };
            Span::styled(text, look)
        }),
    );
    spans.push(Span::styled(tail.to_string(), bar));
    // Its ground, for the row past its words (`draw_bar`).
    Line::from(spans).style(bar)
}

/// The float's box in `area`: fzf's shape — 90% of the width, 80% of the
/// height, centred — never narrower than a list's columns need, nor bigger
/// than `area`. In `u32`, so no width overflows.
fn float_rect(area: Rect) -> Rect {
    let part = |of: u16, tenths: u32, least: u16| {
        let share = u16::try_from(u32::from(of) * tenths / 10).unwrap_or(of);
        share.max(least).min(of)
    };
    let (width, height) = (part(area.width, 9, 60), part(area.height, 8, 12));
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// The float's inner width from which the file is shown beside the list —
/// the list's columns (38) and a readable file.
const PREVIEW_FROM: u16 = 90;

/// **The page's rows for a box `width` cells wide in a frame `tall` rows
/// high** — folded, then without its blank lines if that is what fits, then
/// cut with a last row saying so. Three rows are the box's: two borders and
/// the legend.
fn fitted(lines: &[Line<'static>], width: usize, tall: u16, theme: &Theme) -> Vec<Line<'static>> {
    let room = (tall as usize).saturating_sub(3);
    let fold_all = |lines: &mut dyn Iterator<Item = &Line<'static>>| -> Vec<Line<'static>> {
        lines.flat_map(|line| fold(line, width)).collect()
    };
    let mut rows = fold_all(&mut lines.iter());
    if rows.len() > room {
        let blank = |line: &&Line<'static>| line.spans.iter().all(|s| s.content.trim().is_empty());
        rows = fold_all(&mut lines.iter().filter(|line| !blank(line)));
    }
    if rows.len() > room && room > 0 {
        rows.truncate(room - 1);
        rows.push(Line::from(Span::styled(
            "… more below — a taller terminal shows it",
            theme.ui(Ink::Caution),
        )));
    }
    rows
}

/// **`line` folded to `width` cells** at spaces, each continuation indented
/// under where the line's text began — past a leading label span (`▸ Save`),
/// or as far as the line's own indent — so a description column stays a
/// column. Folded here rather than by `Wrap`, so the rows drawn are the rows
/// counted.
///
/// **A word wider than a row is broken at the row's edge**, and goes on
/// under the same column. It was cut by the row: the trust screen's folder
/// is one word, and run from a deep directory it showed
/// `~/…/worktrees/param-default-b` — a name that reads complete, on the
/// screen that asks whether to trust it.
fn fold(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let total: usize = line.spans.iter().map(|s| s.content.width()).sum();
    if total <= width || width == 0 {
        return vec![line.clone()];
    }
    let hang = match line.spans.as_slice() {
        [label, _, ..] if label.content.ends_with(' ') => label.content.width(),
        [first, ..] => first.content.len() - first.content.trim_start_matches(' ').len(),
        [] => 0,
    }
    .min(width / 2);
    let mut out = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    let mut next_row = |row: &mut Vec<Span<'static>>, used: &mut usize| {
        out.push(Line::from(std::mem::take(row)));
        row.push(Span::raw(" ".repeat(hang)));
        *used = hang;
    };
    for span in &line.spans {
        for piece in span.content.split_inclusive(' ') {
            let word = piece.trim_end_matches(' ').width();
            if used > hang && used + word > width {
                next_row(&mut row, &mut used);
            }
            if used + word <= width {
                row.push(Span::styled(piece.to_string(), span.style));
                used += piece.width();
                continue;
            }
            // The word is broken; the spaces after it go with its last
            // piece, as a whole word's do.
            let (body, gap) = piece.split_at(piece.trim_end_matches(' ').len());
            let mut part = String::new();
            for c in body.chars() {
                let cells = c.width().unwrap_or(0);
                // `used > hang`: a fresh row takes a character whatever its
                // width, or a wide one in a narrow box left an empty row
                // before every character.
                if used > hang && used + cells > width {
                    row.push(Span::styled(std::mem::take(&mut part), span.style));
                    next_row(&mut row, &mut used);
                }
                part.push(c);
                used += cells;
            }
            part.push_str(gap);
            used += gap.len();
            row.push(Span::styled(part, span.style));
        }
    }
    out.push(Line::from(row));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn text(setup: &Setup) -> String {
        let (lines, legend) = setup.page(74, 40, &Theme::terminal());
        lines
            .iter()
            .chain(std::iter::once(&legend))
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn facts() -> Facts {
        Facts {
            key_path: Some("~/.config/shannon/api_key".into()),
            ..Facts::default()
        }
    }

    /// **The first screen says what each path gives up** — the API fully
    /// featured, Claude Code limited in Steven's words, the demo connected to
    /// nothing — and how to leave it.
    #[test]
    fn the_menu_says_what_each_choice_gives_up() {
        let said = text(&Setup::menu(facts()));
        for words in [
            "Anthropic API",
            "Fully featured.",
            "Claude Code",
            "Limited.",
            "You get the TUI features, but you don't have as",
            "much control",
            "Demo mode",
            "doesn't connect to",
            "Esc leave",
            "Use :setup to return to this screen",
        ] {
            assert!(said.contains(words), "{words:?} in\n{said}");
        }
    }

    /// Moving, choosing by number, choosing by Enter, and leaving.
    #[test]
    fn the_menu_chooses_and_leaves() {
        let mut setup = Setup::menu(facts());
        assert_eq!(
            setup.key(key(KeyCode::Char('3'))),
            Effect::Finish(Choice::Demo)
        );

        let mut setup = Setup::menu(facts());
        setup.key(key(KeyCode::Char('j')));
        assert_eq!(setup.key(key(KeyCode::Enter)), Effect::CheckClaudeCode);
        assert!(text(&setup).contains("Claude Code"));

        let mut setup = Setup::menu(facts());
        for _ in 0..5 {
            setup.key(key(KeyCode::Down));
        }
        assert_eq!(
            setup.key(key(KeyCode::Enter)),
            Effect::Finish(Choice::Demo),
            "the cursor stops at the last choice"
        );

        for leave in [key(KeyCode::Esc), key(KeyCode::Char('q')), ctrl('c')] {
            assert_eq!(Setup::menu(facts()).key(leave), Effect::Leave);
        }
    }

    /// The one row `▸` marks.
    fn marked(setup: &Setup) -> String {
        let said = text(setup);
        let rows: Vec<&str> = said.lines().filter(|l| l.contains('▸')).collect();
        assert_eq!(rows.len(), 1, "one row is marked:\n{said}");
        rows[0].to_string()
    }

    /// **`▸` marks the row Enter would choose, and going back lands on the
    /// screen's own row** — rows found from `Choice::ALL`, where they were
    /// literals (review 21, Reviewer C).
    #[test]
    fn the_marker_follows_the_cursor_and_back_lands_on_its_own_row() {
        let mut setup = Setup::menu(facts());
        assert!(marked(&setup).contains("Anthropic API"));
        setup.key(key(KeyCode::Char('j')));
        assert!(marked(&setup).contains("Claude Code"));
        setup.key(key(KeyCode::Char('j')));
        assert!(marked(&setup).contains("Demo mode"));
        // From the last row, so landing on the first is the API screen's doing.
        setup.key(key(KeyCode::Char('1')));
        setup.key(key(KeyCode::Esc));
        assert!(marked(&setup).contains("Anthropic API"));
        setup.key(key(KeyCode::Char('2')));
        setup.key(key(KeyCode::Esc));
        assert!(marked(&setup).contains("Claude Code"));
    }

    /// **The key is never drawn, never printed** — only how long it is — and a
    /// paste replaces it rather than doubling it.
    #[test]
    fn the_key_is_masked_and_a_paste_replaces_it() {
        let mut setup = Setup::menu(facts());
        setup.key(key(KeyCode::Char('1')));
        let secret = "sk-ant-api03-SECRETVALUE";
        setup.paste(secret);
        setup.paste(&format!("  {secret}\n"));
        let said = text(&setup);
        assert!(!said.contains("SECRETVALUE"), "{said}");
        assert!(
            said.contains(&format!("({} characters)", secret.len())),
            "{said}"
        );
        assert!(!format!("{setup:?}").contains("SECRETVALUE"));
        // A paste moves the cursor to Save: what a first launch pastes for.
        match setup.key(key(KeyCode::Enter)) {
            Effect::SaveKey(Some(saved)) => {
                assert!(!format!("{saved:?}").contains("SECRETVALUE"));
                assert_eq!(saved.expose(), secret);
            }
            other => panic!("{other:?}"),
        }
    }

    /// **Use, Save, Exit — on the key pasted, or else the one in use**
    /// (Steven, 10-04: an `ANTHROPIC_API_KEY` in use could not be saved).
    /// Each says what it would do; nothing pasted and nothing in use, Use and
    /// Save ask for a key.
    #[test]
    fn the_key_screen_uses_saves_or_exits() {
        let in_use = Facts {
            key_path: Some("~/.config/shannon/api_key".into()),
            has_key: true,
            key_from: Some("ANTHROPIC_API_KEY".into()),
            ..Facts::default()
        };
        let on_key_screen = |facts: Facts| {
            let mut setup = Setup::menu(facts);
            setup.key(key(KeyCode::Char('1')));
            setup
        };
        let choose = |setup: &mut Setup, at: KeyAction| {
            for _ in 0..3 {
                setup.key(key(KeyCode::Up));
            }
            for _ in 0..at.index() {
                setup.key(key(KeyCode::Down));
            }
            setup.key(key(KeyCode::Enter))
        };

        // Nothing pasted: the key in use.
        let mut setup = on_key_screen(in_use.clone());
        let said = text(&setup);
        assert!(
            said.contains("in use: the key from ANTHROPIC_API_KEY"),
            "{said}"
        );
        assert!(
            said.contains("write the key in use to ~/.config/shannon/api_key"),
            "{said}"
        );
        assert_eq!(choose(&mut setup, KeyAction::Use), Effect::UseKey(None));
        let mut setup = on_key_screen(in_use.clone());
        assert_eq!(choose(&mut setup, KeyAction::Save), Effect::SaveKey(None));
        let mut setup = on_key_screen(in_use.clone());
        assert_eq!(choose(&mut setup, KeyAction::Exit), Effect::Leave);
        let saved = Facts {
            key_is_saved: true,
            ..in_use.clone()
        };
        assert!(text(&on_key_screen(saved)).contains("saved there already"));

        // Pasted: the pasted key, not the one in use.
        let mut setup = on_key_screen(in_use.clone());
        setup.paste("sk-ant-new");
        assert!(text(&setup).contains("write this key to"));
        assert!(matches!(
            choose(&mut setup, KeyAction::Use),
            Effect::UseKey(Some(k)) if k.clone().expose() == "sk-ant-new"
        ));

        // Nothing anywhere: asked for.
        let mut setup = on_key_screen(facts());
        assert_eq!(choose(&mut setup, KeyAction::Save), Effect::None);
        assert!(text(&setup).contains("paste a key first"));
    }

    /// **The cursor follows the field's first and last character**: a key
    /// arriving lands it on Save, the field emptied on Use; between, it stays
    /// where it was put.
    #[test]
    fn the_key_cursor_follows_the_field() {
        let mut setup = Setup::menu(facts());
        setup.key(key(KeyCode::Char('1')));
        assert!(marked(&setup).contains("Use"));
        setup.key(key(KeyCode::Char('s')));
        assert!(marked(&setup).contains("Save"));
        setup.key(key(KeyCode::Up));
        setup.key(key(KeyCode::Char('k')));
        assert!(marked(&setup).contains("Use"), "typing on leaves it be");
        setup.key(ctrl('u'));
        assert!(marked(&setup).contains("Use"));
        setup.paste("sk-ant-x");
        assert!(marked(&setup).contains("Save"));
    }

    /// The frame's rows, as text.
    fn drawn(setup: &Setup, width: u16, height: u16) -> Vec<String> {
        let mut term =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        term.draw(|f| draw(f, setup, false, &Theme::terminal()))
            .unwrap();
        let buf = term.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    /// **A screen keeps the theme's ground** — `Clear` resets a cell to the
    /// terminal's own, so a setup screen over a themed frame drew on the
    /// terminal's background (`clear`).
    #[test]
    fn a_screen_keeps_the_themes_ground() {
        use ratatui::style::Color;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("t.toml"),
            "\"ui.background\" = { bg = \"#1e1e2e\" }\n",
        )
        .unwrap();
        let (theme, _) = Theme::load("t", Some(dir.path())).expect("loads");
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 30)).unwrap();
        term.draw(|f| draw(f, &Setup::menu(facts()), false, &theme))
            .unwrap();
        let buf = term.backend().buffer();
        for (x, y) in [(0, 0), (40, 15)] {
            assert_eq!(
                buf[(x, y)].style().bg,
                Some(Color::Rgb(0x1e, 0x1e, 0x2e)),
                "({x}, {y})"
            );
        }
    }

    /// **A screen's legend draws its keys as keys** — `j/k`, `Enter`, `1–3`
    /// and `Esc` in what-you-type's colour, the words in the legend's own
    /// (`paint::hint`, every row of keys' painter).
    #[test]
    fn a_screens_legend_draws_its_keys_as_keys() {
        let theme = Theme::terminal();
        let (_, legend) = Setup::menu(facts()).page(76, 30, &theme);
        let code = crate::paint::Painter::new(&theme, crate::theme::Context::Code)
            .style(&crate::paint::Run::role(
                "x",
                crate::style::CellStyle::ink(Ink::HelpCode),
            ))
            .fg;
        let keys: Vec<&str> = (legend.spans.iter())
            .filter(|span| span.style.fg == code)
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(keys, ["j/k", "Enter", "1–3", "Esc"], "{legend:?}");
    }

    /// **A bar's keys are underlined in the bar's own colours**, its fill
    /// unbroken: `ui.help.code` cut a hole at each key, and over the fill
    /// it cannot be read (review stop 17, A M3).
    #[test]
    fn a_bars_keys_keep_its_fill() {
        let theme = Theme::terminal();
        let bar = theme.ui(Ink::Bar);
        assert!(bar.bg.is_some(), "premise: the bar is a fill");
        let line = bar_of_keys("Enter edit · x disable", " ", &theme);
        for span in &line.spans {
            assert_eq!((span.style.fg, span.style.bg), (bar.fg, bar.bg), "{span:?}");
        }
        let underlined: Vec<&str> = (line.spans.iter())
            .filter(|s| {
                s.style
                    .add_modifier
                    .contains(ratatui::style::Modifier::UNDERLINED)
            })
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(underlined, ["Enter", "x"], "{line:?}");
    }

    /// **A panel's bar is a bar across its row**, waiting or not, and the
    /// shipped one dark on its hue (Steven, 11-09, image 47: the fill
    /// stopped where the words did, and the terminal's white on its cyan
    /// was hard to read).
    #[test]
    fn a_panels_bar_fills_its_row_dark_on_its_hue() {
        let theme = Theme::terminal();
        let panel = crate::effort_panel::Panel::new(
            shannon_core::EpisodeId::from_seed(b"a session"),
            "claude-sonnet-4-5".into(),
            None,
            None,
        );
        for (waiting, ink) in [(false, Ink::Bar), (true, Ink::BarWaiting)] {
            let ground = theme.ui(ink);
            let black = Some(ratatui::style::Color::Black);
            assert_eq!(ground.fg, black, "{ink:?}: dark on its hue");
            let mut term =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 12)).unwrap();
            let status = Rect::new(0, 11, 120, 1);
            term.draw(|f| draw_panel(f, &panel, status, waiting, &theme))
                .unwrap();
            let buf = term.backend().buffer();
            let words = (0..120).map(|x| buf[(x, 11)].symbol()).collect::<String>();
            assert!(
                words.trim_end().len() < 100,
                "premise: words end short: {words}"
            );
            for x in [0, 60, 119] {
                assert_eq!(buf[(x, 11)].style().bg, ground.bg, "{ink:?} at {x}");
            }
        }
    }

    /// **A screen's "a decision is waiting" is a bar across its row** — the
    /// third bar site, which stopped at its words (stop 17, V2b L5). Its
    /// legend otherwise is plain words, and stays so.
    #[test]
    fn a_screens_waiting_foot_fills_its_row() {
        let theme = Theme::terminal();
        let setup = Setup::menu(facts());
        {
            let ground = theme.ui(Ink::BarWaiting);
            let mut term =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
            term.draw(|f| draw(f, &setup, true, &theme)).unwrap();
            let buf = term.backend().buffer();
            // The box is 76 wide, centred (`draw`): its border at 22 and 97,
            // the foot the row above its bottom edge.
            let bottom = (0..40u16)
                .find(|&y| buf[(22, y)].symbol() == "└")
                .expect("premise: the box");
            let foot = bottom - 1;
            let words: String = (23..97).map(|x| buf[(x, foot)].symbol()).collect();
            assert!(
                words.trim_end().len() < words.len(),
                "premise: room past the words: {words}"
            );
            for x in 23..97 {
                assert_eq!(buf[(x, foot)].style().bg, ground.bg, "at {x}: {words}");
            }
        }
    }

    /// **A rule stands above a panel's lines**, the full width, as vim's
    /// status line stands above `:ls` (Steven, 11-09: "a separator will make
    /// the panel look cleaner") — and gives its row to a line when there is
    /// room for one row only.
    #[test]
    fn a_rule_stands_above_a_panels_lines() {
        let theme = Theme::terminal();
        let panel = crate::effort_panel::Panel::new(
            shannon_core::EpisodeId::from_seed(b"a session"),
            "claude-sonnet-4-5".into(),
            None,
            None,
        );
        let lines = panel.lines(&theme).len() - 1;
        let drawn = |height: u16| {
            let mut term =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, height)).unwrap();
            let status = Rect::new(0, height - 1, 40, 1);
            term.draw(|f| draw_panel(f, &panel, status, false, &theme))
                .unwrap();
            term.backend().buffer().clone()
        };
        let buf = drawn(12);
        let rule = 11 - lines as u16 - 1;
        let row = |buf: &ratatui::buffer::Buffer, y: u16| {
            (0..40).map(|x| buf[(x, y)].symbol()).collect::<String>()
        };
        assert_eq!(row(&buf, rule), "─".repeat(40), "the rule");
        let border = theme.ui(Ink::BorderFocused).fg;
        assert_eq!(
            buf[(20, rule)].style().fg,
            border,
            "in the focused border's colour"
        );
        assert!(
            row(&buf, rule + 1).contains(":effort"),
            "the lines under it"
        );
        // One row above the bar: a line, no rule.
        let tight = drawn(2);
        assert!(row(&tight, 0).contains(":effort"), "{}", row(&tight, 0));
    }

    /// **What does not fit is said, never silently cut** (Steven, 10-04: a
    /// short pane lost the menu's `:setup` line). Tight, the blank lines go
    /// first; tighter, the last row says more is below; narrow, a line folds
    /// under its own column instead of losing its end. The legend stays.
    #[test]
    fn a_small_frame_folds_and_says_what_it_cut() {
        let menu = Setup::menu(facts());
        let rows = |w, h| drawn(&menu, w, h).join("\n");
        let roomy = rows(76, 30);
        assert!(roomy.contains("Use :setup to return"), "{roomy}");
        assert!(!roomy.contains("more below"), "{roomy}");
        // Without its blank lines, it fits where it did not.
        let tight = rows(76, 17);
        assert!(tight.contains("Use :setup to return"), "{tight}");
        assert!(tight.contains("Esc leave"), "{tight}");
        let tighter = rows(76, 9);
        assert!(tighter.contains("more below"), "{tighter}");
        assert!(tighter.contains("Esc leave"), "the legend stays: {tighter}");
        let narrow = rows(44, 40);
        for word in ["unredacted", "login", "Needs", "agent"] {
            assert!(narrow.contains(word), "{word} folded, not cut: {narrow}");
        }
        // Folded under the description column, not back at the edge.
        let fold = drawn(&menu, 44, 40)
            .into_iter()
            .find(|row| row.contains("unredacted"))
            .expect("a folded row");
        assert!(fold.starts_with("│      "), "{fold:?}");
    }

    /// **A folder too long for the box is broken, never cut** — the trust
    /// screen asks about it by name, and a cut name reads as a complete one
    /// (a deep worktree's path did, and the test naming it failed).
    #[test]
    fn a_folder_too_long_for_the_box_is_shown_whole() {
        let long = format!("~/{}end-of-the-name", "deep/".repeat(30));
        let setup = Setup::trust(
            std::path::PathBuf::from("/x"),
            long.clone(),
            false,
            Vec::new(),
        );
        let rows = drawn(&setup, 76, 40);
        let text: String = rows
            .iter()
            .map(|row| row.trim_matches(['│', ' ']))
            .collect();
        assert!(text.contains(&long), "{rows:#?}");
        // Each piece under the folder's own column.
        let pieces: Vec<&String> = rows.iter().filter(|r| r.contains("deep/")).collect();
        assert!(pieces.len() > 1, "{rows:#?}");
        for piece in pieces {
            assert!(
                piece.starts_with("│  ") && !piece.starts_with("│   "),
                "{piece:?}"
            );
        }
    }

    /// A character wider than the row still takes a row of its own — never
    /// an empty row before it.
    #[test]
    fn a_wide_character_in_a_narrow_row_leaves_no_empty_row() {
        let rows = fold(&Line::from("界界界 x"), 1);
        let text = |row: &Line| {
            row.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        };
        assert!(
            rows.iter().all(|row| !text(row).trim().is_empty()),
            "{rows:?}"
        );
        assert_eq!(
            rows.iter().map(text).collect::<String>().replace(' ', ""),
            "界界界x"
        );
    }

    /// An empty key is not saved; a key that does not look like one is
    /// saved, with a warning first; `Esc` goes back to the menu.
    #[test]
    fn the_key_screen_warns_and_goes_back() {
        let mut setup = Setup::menu(facts());
        setup.key(key(KeyCode::Char('1')));
        assert_eq!(setup.key(key(KeyCode::Enter)), Effect::None);
        assert!(text(&setup).contains("paste a key first"));
        for c in "abc".chars() {
            setup.key(key(KeyCode::Char(c)));
        }
        assert!(text(&setup).contains("doesn't look like an Anthropic key"));
        setup.key(ctrl('u'));
        assert!(text(&setup).contains("(nothing yet)"));
        assert_eq!(setup.key(key(KeyCode::Esc)), Effect::None);
        assert!(text(&setup).contains("How should Shannon reach a model?"));
    }

    /// **Claude Code goes on only when it can serve a turn** — or when the
    /// check could not tell, which is not a no — and says how to get there.
    #[test]
    fn claude_code_goes_on_only_when_ready() {
        let mut setup = Setup::menu(facts());
        assert_eq!(setup.key(key(KeyCode::Char('2'))), Effect::CheckClaudeCode);
        setup.set_status(LoginStatus::NotLoggedIn);
        assert_eq!(setup.key(key(KeyCode::Enter)), Effect::None);
        let said = text(&setup);
        assert!(
            said.contains("run `claude` in a terminal and log in"),
            "{said}"
        );
        assert!(said.contains("CLAUDE.md"), "{said}");
        // Its hook's limit, said where you choose it (Steven, 10-22) — the
        // number from the deadline, not typed (review stop 12, T7).
        let minutes = shannon_core::backend::claude_code::HOOK_DEADLINE.as_secs() / 60;
        assert!(
            said.contains(&format!(
                "A call left undecided for {minutes} minutes is refused"
            )),
            "{said}"
        );
        assert!(!said.contains("Enter continue"), "{said}");
        assert_eq!(setup.key(key(KeyCode::Char('r'))), Effect::CheckClaudeCode);
        setup.set_status(LoginStatus::LoggedIn("claude.ai".into()));
        assert!(text(&setup).contains("Logged in (claude.ai)."));
        assert_eq!(
            setup.key(key(KeyCode::Enter)),
            Effect::Finish(Choice::ClaudeCode)
        );

        setup.set_status(LoginStatus::Unknown("old".into()));
        assert_eq!(
            setup.key(key(KeyCode::Enter)),
            Effect::Finish(Choice::ClaudeCode)
        );
        setup.set_status(LoginStatus::NotInstalled);
        assert!(text(&setup).contains("not installed"));
    }

    /// **Opened by a model switch, it says why and leaves to the session** —
    /// nobody asked for the menu.
    #[test]
    fn opened_by_a_model_switch_it_says_why_and_esc_leaves() {
        let why = "`opus` answers from the Anthropic API, which needs a key".to_string();
        let mut setup = Setup::at(Choice::Api, why.clone(), facts(), None);
        assert!(text(&setup).contains(&why));
        assert_eq!(setup.entry(), Entry::Model);
        assert_eq!(setup.key(key(KeyCode::Esc)), Effect::Leave);

        let mut setup = Setup::at(
            Choice::ClaudeCode,
            "needs a login".into(),
            facts(),
            Some(LoginStatus::NotLoggedIn),
        );
        assert!(text(&setup).contains("Not logged in"));
        assert_eq!(setup.key(key(KeyCode::Esc)), Effect::Leave);
    }

    /// The facts the key screen states: nowhere to save, a variable read
    /// first, a key already in use.
    #[test]
    fn the_key_screen_states_what_will_happen() {
        let mut setup = Setup::menu(Facts {
            key_path: None,
            key_overridden_by: Some("ANTHROPIC_API_KEY"),
            has_key: true,
            ..Facts::default()
        });
        setup.key(key(KeyCode::Char('1')));
        let said = text(&setup);
        assert!(said.contains("nowhere to save it"), "{said}");
        assert!(said.contains("ANTHROPIC_API_KEY is set"), "{said}");
        assert!(
            said.contains("keep answering from the key in use"),
            "{said}"
        );
        // …and it does: nothing pasted, a key in use, Enter keeps it.
        assert_eq!(setup.key(key(KeyCode::Enter)), Effect::UseKey(None));
    }
}
