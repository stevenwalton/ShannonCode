//! **A bare `:effort`** — vim's command-line window (`q:`, `q?`) for one
//! command (Steven, 10-04). A short split at the bottom, the session still
//! readable above it: one line per `:effort` you could type, the cursor on
//! this session's, `<CR>` runs the line under it, `q` / `<Esc>` close it
//! having run nothing.
//!
//! `:models` is the whole screen for model, effort and context together; this
//! is the one knob, where you were. Routed and drawn as a screen of
//! `crate::setup` — one answer to "does it take this key" — drawn as a panel.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use shannon_core::backend::{claude_code, models};
use shannon_core::{Effort, EpisodeId};

use crate::setup::Effect;
use crate::style::Ink;
use crate::theme::Theme;

/// The effort choices, in order: `default` (follow the run's), then every
/// level — built from [`Effort::ALL`]. The panel's lines and `:models`'
/// effort row; one list, so the two cannot offer different ones.
pub(crate) fn choices() -> Vec<Option<Effort>> {
    std::iter::once(None).chain(Effort::ALL.map(Some)).collect()
}

/// A choice as `:effort` spells it.
pub(crate) fn word(choice: Option<Effort>) -> &'static str {
    choice.map_or("default", Effort::as_str)
}

/// **What `default` does for a session on `model`** (`name` as the user reads
/// it), where the run's `[turn] effort` is `run` — the one sentence for it.
/// Its three consumers: this panel's `default` line, `:models`' effort note,
/// and `:effort default`'s notice. They were three copies, and they had
/// already come apart: on Haiku the panel said the run's level was followed
/// while the picker said none was sent (Reviewers, 10-04).
pub(crate) fn default_story(model: &str, name: &str, run: Option<Effort>) -> String {
    if models::efforts(model).is_some_and(<[Effort]>::is_empty) {
        return format!("{name} takes none — sent none");
    }
    match run {
        Some(run) if models::effort_for(model, Some(run)).is_none() => {
            format!("follows [turn] effort: {run}, which {name} does not take — sent none")
        }
        Some(run) => format!("follows [turn] effort: {run}"),
        None if claude_code::is_claude_code(model) => {
            "sends none: Claude Code's own (your /effort)".to_string()
        }
        None => match models::default_effort(model) {
            Some(own) => format!("sends none: {name} works at {own}"),
            None => "sends none: the model's own".to_string(),
        },
    }
}

/// `:effort`'s panel, open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    /// The session it opened on — what `<CR>` sets, and only that.
    episode: EpisodeId,
    /// What the bar calls it: its short id, or its name too
    /// ([`labelled`](Self::labelled)).
    tag: String,
    /// The model that answers it — what the notes are about.
    model: String,
    /// The session's own effort; `None` follows the run's.
    now: Option<Effort>,
    /// The run's `[turn] effort`, what `default` follows.
    run: Option<Effort>,
    /// The cursor, in [`choices`].
    at: usize,
}

impl Panel {
    /// The panel for `episode` on `model`, the cursor on its effort `now`.
    pub fn new(
        episode: EpisodeId,
        model: String,
        now: Option<Effort>,
        run: Option<Effort>,
    ) -> Panel {
        // Every `Option<Effort>` is a line, so this always finds one.
        let at = choices().iter().position(|c| *c == now).unwrap_or(0);
        Panel {
            episode,
            tag: episode.short_hex(),
            model,
            now,
            run,
            at,
        }
    }

    /// The session's name, if it has one, on the bar beside its id.
    pub fn labelled(mut self, label: &shannon_core::Label) -> Panel {
        self.tag = label.tag(self.episode);
        self
    }

    /// One key: vim's motions over the lines, `<CR>` runs one. `Ctrl-C` is
    /// setup's, which closes from any screen.
    ///
    /// **Only plain keys** (and shifted, for `G`): `key.code` alone made
    /// `<C-k>` a `k` and `<C-q>` a `q` — so `<C-w>k`, reached for out of
    /// habit, moved the cursor, and an Enter after it set a level nobody
    /// chose (Reviewer F).
    pub fn key(&mut self, key: KeyEvent) -> Effect {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return Effect::None;
        }
        let last = choices().len() - 1;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Effect::Leave,
            KeyCode::Enter => {
                return Effect::Effort {
                    episode: self.episode,
                    effort: choices()[self.at],
                };
            }
            KeyCode::Down | KeyCode::Char('j') => self.at = (self.at + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => self.at = self.at.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => self.at = 0,
            KeyCode::Char('G') | KeyCode::End => self.at = last,
            _ => {}
        }
        Effect::None
    }

    /// The line the cursor is on, in [`lines`](Self::lines).
    pub fn cursor(&self) -> usize {
        self.at
    }

    /// **The lines**, the bar last — vim's `[Command Line]`, its keys first
    /// so a narrow terminal cuts the label rather than the way out.
    pub fn lines(&self, theme: &Theme) -> Vec<Line<'static>> {
        let dim = theme.ui(Ink::TextQuiet);
        let takes = models::efforts(&self.model);
        let name = models::short_name(&self.model).to_string();
        let mut out: Vec<Line<'static>> = choices()
            .into_iter()
            .enumerate()
            .map(|(i, choice)| {
                let here = i == self.at;
                let open = match (choice, takes) {
                    (Some(level), Some(levels)) => levels.contains(&level),
                    _ => true,
                };
                let style = match (here, open) {
                    (true, _) => theme.ui(Ink::Chosen),
                    (false, true) => Style::default(),
                    (false, false) => dim,
                };
                let mut spans = vec![
                    Span::styled(if here { "▸" } else { " " }, style),
                    Span::styled(format!(":effort {:<8}", word(choice)), style),
                ];
                let note = self.note(choice, open, &name);
                if !note.is_empty() {
                    spans.push(Span::styled(format!("  {note}"), dim));
                }
                Line::from(spans)
            })
            .collect();
        // Its keys drawn as keys (`setup::bar_of_keys`), the note after them
        // in the bar's words.
        let note = format!(" — effort for {} · {name} ", self.tag);
        out.push(crate::setup::bar_of_keys(
            "q close · Enter set · j/k move",
            &note,
            theme,
        ));
        out
    }

    /// What one line means here: whose it is now, what `default` does, and
    /// whether the session's model takes it.
    fn note(&self, choice: Option<Effort>, open: bool, name: &str) -> String {
        let mut said = Vec::new();
        if choice == self.now {
            said.push("this session's".to_string());
        }
        match choice {
            None => said.push(default_story(&self.model, name, self.run)),
            Some(_) if !open => said.push(format!("{name} takes none — sent none")),
            Some(_) => {}
        }
        said.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode() -> EpisodeId {
        EpisodeId::from_seed(b"the session")
    }

    fn panel(model: &str, now: Option<Effort>, run: Option<Effort>) -> Panel {
        Panel::new(episode(), model.into(), now, run)
    }

    fn press(p: &mut Panel, keys: &str) -> Effect {
        let mut last = Effect::None;
        for c in keys.chars() {
            let code = match c {
                '\n' => KeyCode::Enter,
                c => KeyCode::Char(c),
            };
            // A capital with SHIFT, as a terminal sends it (review stop 12, T7).
            let mods = match c.is_ascii_uppercase() {
                true => KeyModifiers::SHIFT,
                false => KeyModifiers::NONE,
            };
            last = p.key(KeyEvent::new(code, mods));
        }
        last
    }

    fn set(effort: Option<Effort>) -> Effect {
        Effect::Effort {
            episode: episode(),
            effort,
        }
    }

    fn text(p: &Panel) -> String {
        p.lines(&Theme::terminal())
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// **It opens on this session's line**, and `<CR>` sets that session.
    #[test]
    fn it_opens_on_the_sessions_line() {
        let mut p = panel("claude-opus-5-5", Some(Effort::Xhigh), None);
        let page = text(&p);
        let here = page.lines().find(|l| l.starts_with('▸')).unwrap();
        assert!(
            here.contains(":effort xhigh") && here.contains("this session's"),
            "{page}"
        );
        assert_eq!(press(&mut p, "\n"), set(Some(Effort::Xhigh)));
    }

    /// vim's motions, clamped at both ends; `q` runs nothing.
    #[test]
    fn the_motions_are_vims() {
        let mut p = panel("claude-opus-5-5", None, None);
        assert_eq!(press(&mut p, "k\n"), set(None), "clamped at the top");
        assert_eq!(press(&mut p, "jj\n"), set(Some(Effort::Medium)));
        assert_eq!(press(&mut p, "G\n"), set(Some(Effort::Max)));
        assert_eq!(press(&mut p, "j\n"), set(Some(Effort::Max)), "clamped");
        assert_eq!(press(&mut p, "g\n"), set(None));
        assert_eq!(press(&mut p, "jq"), Effect::Leave);
    }

    /// **A modified key is not its letter**: `<C-k>` does not move the
    /// cursor, `<C-q>` does not close — out of habit, `<C-w>k` was a `k`.
    #[test]
    fn a_modified_key_is_not_its_letter() {
        let mut p = panel("claude-opus-5-5", None, None);
        for c in ['w', 'k', 'j', 'q'] {
            for mods in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
                let effect = p.key(KeyEvent::new(KeyCode::Char(c), mods));
                assert_eq!(effect, Effect::None, "{mods:?}-{c}");
            }
        }
        assert_eq!(press(&mut p, "\n"), set(None), "the cursor never moved");
    }

    /// **`default` says what it does** — follows the run's setting, else the
    /// model's own, for Claude Code the user's own Claude Code setting, and
    /// on a model that takes none, that none is sent whatever the run says.
    #[test]
    fn default_says_what_it_does() {
        let has = |p: &Panel, said: &str| {
            assert!(text(p).contains(said), "`{said}` in:\n{}", text(p));
        };
        has(
            &panel("claude-opus-5-5", None, None),
            "sends none: opus works at medium",
        );
        let cc = panel("claude-code/opus", None, None);
        has(&cc, "Claude Code's own");
        assert!(!text(&cc).contains("works at medium"), "{}", text(&cc));
        has(
            &panel("claude-opus-5-5", None, Some(Effort::High)),
            "follows [turn] effort: high",
        );
        let haiku = panel("claude-haiku-4-5-20251001", None, Some(Effort::High));
        has(&haiku, "haiku-4-5 takes none — sent none");
        assert!(!text(&haiku).contains("follows"), "{}", text(&haiku));
    }

    /// A model that takes no effort says so on every level, drawn dim.
    #[test]
    fn a_model_without_effort_says_so() {
        let p = panel("claude-haiku-4-5-20251001", None, None);
        assert!(text(&p).contains("haiku-4-5 takes none"), "{}", text(&p));
        let theme = Theme::terminal();
        let max = p
            .lines(&theme)
            .into_iter()
            .find(|l| l.to_string().contains(":effort max"))
            .unwrap();
        assert_eq!(max.spans[1].style, theme.ui(Ink::TextQuiet));
    }

    /// **The way out survives a narrow terminal**: the bar's keys come first,
    /// within 40 cells, for every model `:models` offers.
    #[test]
    fn the_bar_leads_with_its_keys() {
        let bar = panel("claude-code/claude-fable-5-1", None, None)
            .lines(&Theme::terminal())
            .pop()
            .unwrap()
            .to_string();
        let keys = bar.find("q close").expect("the way out");
        assert!(keys < 4 && bar.find("Enter set").unwrap() < 40, "{bar}");
    }
}
