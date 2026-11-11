//! **`:models`** — which model a session talks to, how hard it is asked to
//! work, and the context budget it works inside, on one screen (Steven, 10-04:
//! "select from available models, effort, and context size").
//!
//! **Two scopes, and the screen says which.** The model and the effort are
//! this session's, kept with it (`:model`, `:effort`); the budget is the
//! run's, as `:set` sets it — forgotten at quit unless your config says it.
//! Enter applies through exactly those doors (`App::apply_models`), so the
//! screen can do nothing they would refuse, and it says before Enter what
//! Enter changes.
//!
//! Drawn and routed as a screen of `crate::setup` — one overlay, one answer to
//! "does it take this key" — with its state and its page here.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use shannon_core::backend::{claude_code, models};
use shannon_core::{Effort, EpisodeId};

use crate::effort_panel::{choices as efforts, default_story, word};
use crate::fold::{self, FoldVerb};
use crate::setup::Effect;
use crate::style::Ink;
use crate::theme::Theme;

/// What the screen hands the app on Enter: each field as chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    /// The session's model, as `:model` takes it.
    pub model: String,
    /// The session's own effort — `None` follows the run's `[turn] effort`.
    pub effort: Option<Effort>,
    /// `turn.max_context_tokens` — `0` is the model's own window.
    pub budget: u32,
}

/// The field the cursor is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Model,
    Effort,
    Context,
}

impl Field {
    const ALL: [Field; 3] = [Field::Model, Field::Effort, Field::Context];

    fn index(self) -> usize {
        Self::ALL.iter().position(|f| *f == self).unwrap_or(0)
    }
}

/// The budgets offered: the model's own window (`0`), then three bounds.
const BUDGETS: [u32; 4] = [0, 500_000, 200_000, 100_000];

/// The width of a group's rule, label included.
const RULE: usize = 56;

/// The page's width inside the box — what no line may pass
/// (`no_line_of_the_page_folds`).
const PAGE: usize = 74;

/// Where a field's note starts: under its options, past the label.
const NOTE: &str = "           ";

/// One model the screen offers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Offer {
    /// What `:model` is given.
    id: String,
    /// What the row calls it.
    name: String,
    /// Superseded — kept in the older models fold ([`models::Model::older`]).
    older: bool,
}

/// **The models offered**, a group per route: the alias table through the
/// API, the older ones in a fold; Claude Code's by the names `claude
/// --model` takes — Fable and Haiku by their ids, which pin the latest
/// where the alias can resolve to an older one (a gateway session,
/// `ANTHROPIC_DEFAULT_FABLE_MODEL`; a Claude Code whose `haiku` is still
/// Haiku 4.5, measured against Haiku 5.5's 1M window: a gauge reading
/// emptier than it is); then the demo stub.
fn offers() -> Vec<Offer> {
    let api = models::ALIASES
        .iter()
        .filter(|m| m.id != models::STUB)
        .map(|m| Offer {
            id: m.id.to_string(),
            name: m.alias.to_string(),
            older: m.older,
        });
    let claude = [
        ("haiku", "claude-haiku-5-5"),
        ("sonnet", "sonnet"),
        ("opus", "opus"),
        ("fable", "claude-fable-5-1"),
    ]
    .into_iter()
    .map(|(name, arg)| Offer {
        id: format!("{}{arg}", claude_code::MODEL_PREFIX),
        name: name.to_string(),
        older: false,
    });
    let stub = std::iter::once(Offer {
        id: models::STUB.to_string(),
        name: "stub".to_string(),
        older: false,
    });
    api.chain(claude).chain(stub).collect()
}

/// **A row of the model list** — a model, or the line the older models
/// fold shuts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    /// The offer at this index.
    Offer(usize),
    /// The older models' fold line.
    Older,
}

/// **`id` as Enter and the effort note say it** — the model itself, not its
/// row's short name (Steven, 10-31: `sonnet` beside `sonnet-5` said which was
/// Sonnet 5.5 only after Enter, "post hoc information and we need some a
/// priori"): the API's id, undated as the API also takes it; for Claude Code,
/// what `claude --model` is given, which Claude Code resolves; the stub by
/// its name.
fn said(id: &str) -> String {
    if id == models::STUB {
        return "stub".to_string();
    }
    let given = id.strip_prefix(claude_code::MODEL_PREFIX).unwrap_or(id);
    models::undated(given).unwrap_or(given).to_string()
}

/// `text` from `first` on, word-wrapped to the page, its later lines under
/// its first's text — a note or a change longer than the page is two lines,
/// never one the box folds at its edge.
fn fitted(first: &str, text: &str, style: Style) -> Vec<Line<'static>> {
    let indent = crate::wrap::str_cells(first);
    let chars: Vec<char> = text.chars().collect();
    crate::wrap::wrap_cells_word(&chars, PAGE.saturating_sub(indent))
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let lead = if i == 0 {
                first.to_string()
            } else {
                " ".repeat(indent)
            };
            Line::from(Span::styled(format!("{lead}{}", row.text), style))
        })
        .collect()
}

/// How a session on `id` is answered — the group its row is under.
fn via(id: &str) -> &'static str {
    if claude_code::is_claude_code(id) {
        "Claude Code"
    } else if id == models::STUB {
        "demo"
    } else {
        "API"
    }
}

/// A token count in the space a column has: `1M`, `200k`, `4k`.
fn tokens(n: u32) -> String {
    if n >= 1_000_000 && n.is_multiple_of(1_000_000) {
        format!("{}M", n / 1_000_000)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

/// The effort levels `id` takes, as its row says them, and what it does when
/// sent none: the model's default through the API, the user's own Claude
/// Code setting through Claude Code (`models::default_effort` says why).
/// A range only where it is one: Opus 4.6 takes `max` but not `xhigh`, and
/// `low–max` said it took both.
fn effort_range(id: &str) -> String {
    let default = if claude_code::is_claude_code(id) {
        " · your /effort".to_string()
    } else {
        models::default_effort(id).map_or(String::new(), |e| format!(" · {e}"))
    };
    let unbroken = |levels: &[Effort]| {
        let at = |e: &Effort| Effort::ALL.iter().position(|l| l == e);
        levels
            .windows(2)
            .all(|w| at(&w[1]) == at(&w[0]).map(|i| i + 1))
    };
    match models::efforts(id) {
        None => "?".to_string(),
        Some([]) => "—".to_string(),
        Some([only]) => format!("{only}{default}"),
        Some(levels @ [first, .., last]) if unbroken(levels) => {
            format!("{first}–{last}{default}")
        }
        Some(levels) => {
            let said: Vec<&str> = levels.iter().map(|e| e.as_str()).collect();
            format!("{}{default}", said.join("/"))
        }
    }
}

/// `:models`, open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    /// The session it opened on — what Enter applies to, and only that.
    episode: EpisodeId,
    /// What the page calls it: its short id, or its name too
    /// ([`labelled`](Self::labelled)).
    tag: String,
    /// Each field as it was when the screen opened — what Enter compares to.
    now: Chosen,
    /// The run's `[turn] effort`, what `default` follows.
    run: Option<Effort>,
    offers: Vec<Offer>,
    field: Field,
    /// The cursor in the model list — a model, or the older models' line.
    cursor: Row,
    /// The older models fold, open.
    older_open: bool,
    /// A `z` was pressed: the next key is the fold command's
    /// (`[[feedback_chord_owns_its_keys]]`).
    z_pending: bool,
    /// The cursor in [`efforts`].
    effort: usize,
    budgets: Vec<u32>,
    /// The cursor in [`budgets`](Self::budgets).
    budget: usize,
    /// No key, so the stub answers in place of the session's model.
    stub_answers: bool,
}

impl Picker {
    /// The screen for `episode`, whose fields are `now` — `run` is the run's
    /// `[turn] effort`, which its `default` follows — the cursor in `field`.
    ///
    /// **What is, is offered.** A model the list does not name, or a budget
    /// it does not, is the session's all the same: added — a model at the head
    /// of its own route's group — and the cursor put on it, so the screen
    /// opens on the truth, and Enter on an untouched screen changes nothing.
    /// An older model opens its fold, for the same reason.
    pub fn new(episode: EpisodeId, now: Chosen, run: Option<Effort>, field: Field) -> Picker {
        let mut offers = offers();
        let model = match offers.iter().position(|o| o.id == now.model) {
            Some(i) => i,
            None => {
                let at = offers
                    .iter()
                    .position(|o| via(&o.id) == via(&now.model))
                    .unwrap_or(offers.len());
                let name = models::short_name(&now.model).to_string();
                offers.insert(
                    at,
                    Offer {
                        id: now.model.clone(),
                        name,
                        older: false,
                    },
                );
                at
            }
        };
        let older_open = offers[model].older;
        let mut budgets = BUDGETS.to_vec();
        let budget = budgets
            .iter()
            .position(|b| *b == now.budget)
            .unwrap_or_else(|| {
                budgets.push(now.budget);
                budgets.len() - 1
            });
        // Every `Option<Effort>` is in the list, so this always finds one.
        let effort = efforts().iter().position(|e| *e == now.effort).unwrap_or(0);
        Picker {
            episode,
            tag: episode.short_hex(),
            now,
            run,
            offers,
            field,
            cursor: Row::Offer(model),
            older_open,
            z_pending: false,
            effort,
            budgets,
            budget,
            stub_answers: false,
        }
    }

    /// **The stub answers instead** — no key — said under the header: the
    /// header names the session's choice, and a bare `:model`, which said
    /// so, is this screen now (Steven, 10-06).
    pub fn stub_answering(mut self, yes: bool) -> Picker {
        self.stub_answers = yes;
        self
    }

    /// The session's name, if it has one, on the page beside its id.
    pub fn labelled(mut self, label: &shannon_core::Label) -> Picker {
        self.tag = label.tag(self.episode);
        self
    }

    /// Each field as the cursors have it.
    pub fn chosen(&self) -> Chosen {
        Chosen {
            model: self.highlighted().id.clone(),
            effort: efforts()[self.effort],
            budget: self.budgets[self.budget],
        }
    }

    /// **The model the cursor chooses** — its row's, or on the older models'
    /// line, which chooses none, the session's own.
    fn highlighted(&self) -> &Offer {
        match self.cursor {
            Row::Offer(i) => &self.offers[i],
            // Offered by construction (`new`); the first row is never reached.
            Row::Older => self
                .offers
                .iter()
                .find(|o| o.id == self.now.model)
                .unwrap_or(&self.offers[0]),
        }
    }

    /// **The model list as drawn**: each offer, and where the older ones
    /// begin, their fold's line — the offers under it only while it is open.
    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (i, offer) in self.offers.iter().enumerate() {
            if offer.older {
                if !rows.contains(&Row::Older) {
                    rows.push(Row::Older);
                }
                if !self.older_open {
                    continue;
                }
            }
            rows.push(Row::Offer(i));
        }
        rows
    }

    /// How many models the older models fold holds.
    fn older_count(&self) -> usize {
        self.offers.iter().filter(|o| o.older).count()
    }

    /// Whether the cursor is on a model inside the fold.
    fn in_fold(&self) -> bool {
        matches!(self.cursor, Row::Offer(i) if self.offers[i].older)
    }

    /// **A fold command** on the older models — the list's one fold, so every
    /// `z` reaches it wherever the cursor is. Shut, it takes a cursor inside
    /// it to its line, as vim's `zc` does.
    fn fold(&mut self, verb: FoldVerb) {
        self.older_open = verb.apply(self.older_open);
        if !self.older_open && self.in_fold() {
            self.cursor = Row::Older;
        }
    }

    /// One key. `Ctrl-C` is setup's, which leaves from any screen. Only
    /// plain keys (and shifted): `key.code` alone made `<C-q>` a `q`.
    pub fn key(&mut self, key: KeyEvent) -> Effect {
        // The key after a `z` is the fold command's, whatever it is — `z`
        // then `q` does not leave (a chord owns its keys).
        let z = std::mem::take(&mut self.z_pending);
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return Effect::None;
        }
        if z {
            if let Some(verb) = match key.code {
                KeyCode::Char(c) => FoldVerb::after_z(c),
                _ => None,
            } {
                self.fold(verb);
            }
            return Effect::None;
        }
        let on_older = self.field == Field::Model && self.cursor == Row::Older;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Effect::Leave,
            // On the fold's line, which chooses no model, Enter opens or
            // shuts it — netrw's Enter on a directory.
            KeyCode::Enter if on_older => self.fold(FoldVerb::Toggle),
            KeyCode::Enter => {
                return Effect::Models {
                    episode: self.episode,
                    chosen: self.chosen(),
                };
            }
            KeyCode::Char('z') => self.z_pending = true,
            KeyCode::Tab => self.field = Field::ALL[(self.field.index() + 1) % 3],
            KeyCode::BackTab => self.field = Field::ALL[(self.field.index() + 2) % 3],
            KeyCode::Down | KeyCode::Char('j') => self.down(),
            KeyCode::Up | KeyCode::Char('k') => self.up(),
            KeyCode::Left | KeyCode::Char('h') => self.side(-1),
            KeyCode::Right | KeyCode::Char('l') => self.side(1),
            _ => {}
        }
        Effect::None
    }

    /// The cursor `by` rows along the list as drawn; `false` past either end.
    fn step(&mut self, by: isize) -> bool {
        let rows = self.rows();
        let at = rows.iter().position(|r| *r == self.cursor).unwrap_or(0);
        match at.checked_add_signed(by).filter(|n| *n < rows.len()) {
            Some(n) => {
                self.cursor = rows[n];
                true
            }
            None => false,
        }
    }

    /// Down the list, then on into the fields below it.
    fn down(&mut self) {
        match self.field {
            Field::Model => {
                if !self.step(1) {
                    self.field = Field::Effort;
                }
            }
            Field::Effort => self.field = Field::Context,
            Field::Context => {}
        }
    }

    /// Up the fields, then up the list.
    fn up(&mut self) {
        match self.field {
            Field::Model => {
                self.step(-1);
            }
            Field::Effort => self.field = Field::Model,
            Field::Context => self.field = Field::Effort,
        }
    }

    /// Change the effort or the budget a step — the model is chosen by `j/k`.
    /// On the list, `l` opens the older models from their line and `h` shuts
    /// them from it or from inside, as a tree's are.
    fn side(&mut self, step: isize) {
        let (at, len) = match self.field {
            Field::Effort => (&mut self.effort, efforts().len()),
            Field::Context => (&mut self.budget, self.budgets.len()),
            Field::Model => {
                match step > 0 {
                    true if self.cursor == Row::Older => self.fold(FoldVerb::Open),
                    false if self.cursor == Row::Older || self.in_fold() => {
                        self.fold(FoldVerb::Close)
                    }
                    _ => {}
                }
                return;
            }
        };
        *at = at.saturating_add_signed(step).min(len - 1);
    }

    /// `id` as the screen names it: its row's name and its route — `opus`
    /// alone cannot tell the API's from Claude Code's.
    fn spoken(&self, id: &str) -> String {
        let name = self
            .offers
            .iter()
            .find(|o| o.id == id)
            .map_or_else(|| models::short_name(id).to_string(), |o| o.name.clone());
        format!("{name} ({})", via(id))
    }

    /// The session's effort as the screen says it — `default` with what it
    /// follows, when the run says one.
    fn effort_said(&self, effort: Option<Effort>) -> String {
        match (effort, self.run) {
            (None, Some(run)) => format!("default ({run})"),
            (effort, _) => word(effort).to_string(),
        }
    }

    /// **The page** into `out`, and its legend — `crate::setup` boxes it.
    pub fn page(&self, out: &mut Vec<Line<'static>>, theme: &Theme) -> &'static str {
        let dim = theme.ui(Ink::TextQuiet);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let chosen = self.chosen();
        let highlighted = self.highlighted().clone();

        // One row at the box's width for every route (it folded for Claude
        // Code's, whose full id it printed).
        out.push(Line::from(vec![
            Span::styled(format!("Session {}", self.tag), bold),
            Span::styled(
                // `*` marks a budget you chose, as the status line's `ctx 83%*`.
                format!(
                    " — {} · effort {} · ctx {}",
                    self.spoken(&self.now.model),
                    self.effort_said(self.now.effort),
                    match self.now.budget {
                        0 => tokens(models::context_window(&self.now.model)),
                        n => format!("{}*", tokens(n)),
                    },
                ),
                dim,
            ),
        ]));
        if self.stub_answers {
            out.push(Line::from(Span::styled(
                format!(
                    "no API key, so the stub answers, not {} — `:setup` adds one",
                    models::short_name(&self.now.model)
                ),
                theme.ui(Ink::Caution),
            )));
        }
        out.push(Line::from(""));

        out.push(Line::from(vec![
            self.label_span(Field::Model, "Model", theme),
            // Over the rows' columns: their `  ▸ ` and a 14-cell name are 18
            // cells, this label 9.
            Span::styled(
                format!("{:<9}{:>8}{:>7}   effort · default", "", "context", "reply"),
                dim,
            ),
        ]));
        // A rule over each route's group, named (Steven: "a small separator
        // line to distinguish the CC models from API? And stub").
        let mut group = "";
        for row in self.rows() {
            let here = row == self.cursor;
            let style = if here {
                self.focus_style(Field::Model, theme)
            } else {
                Style::default()
            };
            let mark = Span::styled(format!("  {} ", if here { "▸" } else { " " }), style);
            let Row::Offer(i) = row else {
                // The fold's line, where the model names stand: its glyph,
                // and what it holds (Steven's sketch: `> older models`).
                let line = format!(
                    "{} older models ({})",
                    fold::glyph(self.older_open),
                    self.older_count()
                );
                let style = if here { style } else { dim };
                out.push(Line::from(vec![mark, Span::styled(line, style)]));
                continue;
            };
            let offer = &self.offers[i];
            let route = via(&offer.id);
            if route != group {
                group = route;
                let fill = "─".repeat(RULE.saturating_sub(route.len() + 6));
                out.push(Line::from(Span::styled(
                    format!("  ── {route} {fill}"),
                    dim,
                )));
            }
            // A model inside the fold sits under its line, its columns where
            // every other row's are.
            let (indent, name) = if offer.older { ("  ", 12) } else { ("", 14) };
            out.push(Line::from(vec![
                mark,
                Span::styled(
                    format!(
                        "{indent}{:<name$}{:>8}{:>7}   {}",
                        offer.name,
                        tokens(models::context_window(&offer.id)),
                        tokens(models::max_output_tokens(&offer.id)),
                        effort_range(&offer.id),
                    ),
                    style,
                ),
            ]));
        }
        out.push(Line::from(""));

        // Effort: every level, those the highlighted model does not take dim.
        let takes = models::efforts(&highlighted.id);
        let mut row = vec![self.label_span(Field::Effort, "Effort ", theme)];
        for (i, effort) in efforts().into_iter().enumerate() {
            let open = match (effort, takes) {
                (Some(level), Some(levels)) => levels.contains(&level),
                _ => true,
            };
            row.push(self.option(
                Field::Effort,
                i == self.effort,
                word(effort).to_string(),
                open,
                theme,
            ));
        }
        out.push(Line::from(row));
        out.extend(fitted(
            NOTE,
            &self.effort_note(&highlighted.id, chosen.effort),
            dim,
        ));

        let mut row = vec![self.label_span(Field::Context, "Context", theme)];
        for (i, budget) in self.budgets.iter().enumerate() {
            let word = self.budget_word(*budget, &highlighted.id);
            row.push(self.option(Field::Context, i == self.budget, word, true, theme));
        }
        out.push(Line::from(row));
        let window = models::context_window(&highlighted.id);
        let (note, style) = if chosen.budget > window {
            (
                format!(
                    "above {}'s {} window — the gauge would read low",
                    highlighted.name,
                    tokens(window)
                ),
                theme.ui(Ink::Caution),
            )
        } else {
            (
                "the ctx gauge's budget; the model's window is the limit".to_string(),
                dim,
            )
        };
        out.extend(fitted(NOTE, &note, style));
        out.push(Line::from(""));

        out.push(Line::from(Span::styled(
            "Model and effort: this session's. Context: the run's, until you quit.",
            dim,
        )));
        out.extend(self.changes(&chosen, theme));

        match (self.field, self.cursor) {
            (Field::Model, Row::Older) if self.older_open => {
                "Enter, h shut · j/k model · Tab effort, context · Esc leave"
            }
            (Field::Model, Row::Older) => {
                "Enter, l open · j/k model · Tab effort, context · Esc leave"
            }
            (Field::Model, _) => {
                "j/k model · za older · Tab effort, context · Enter apply · Esc leave"
            }
            (Field::Effort | Field::Context, _) => {
                "h/l change · j/k field · Enter apply · Esc leave"
            }
        }
    }

    /// **What Enter would change**, said before it is pressed — a model as
    /// itself ([`said`]: `claude-sonnet-5`, not `sonnet-5`), by its route too
    /// when the route changes (`opus (API) → opus (Claude Code)` read `opus →
    /// opus`). On the older models' line, which chooses no model, what Enter
    /// does there.
    fn changes(&self, chosen: &Chosen, theme: &Theme) -> Vec<Line<'static>> {
        if self.field == Field::Model && self.cursor == Row::Older {
            let what = match self.older_open {
                true => "Enter: shut the older models",
                false => "Enter: open the older models",
            };
            return vec![Line::from(Span::styled(what, theme.ui(Ink::Chosen)))];
        }
        let mut parts = Vec::new();
        if chosen.model != self.now.model {
            let (was, to) = (&self.now.model, &chosen.model);
            parts.push(if via(was) == via(to) {
                format!("model {} → {}", said(was), said(to))
            } else {
                format!(
                    "model {} ({}) → {} ({})",
                    said(was),
                    via(was),
                    said(to),
                    via(to)
                )
            });
        }
        if chosen.effort != self.now.effort {
            parts.push(format!(
                "effort {} → {}",
                self.effort_said(self.now.effort),
                self.effort_said(chosen.effort)
            ));
        }
        if chosen.budget != self.now.budget {
            parts.push(format!(
                "context {} → {}",
                self.budget_word(self.now.budget, &self.now.model),
                self.budget_word(chosen.budget, &chosen.model)
            ));
        }
        if parts.is_empty() {
            vec![Line::from(Span::styled(
                "Enter changes nothing.",
                theme.ui(Ink::TextQuiet),
            ))]
        } else {
            fitted("Enter: ", &parts.join(", "), theme.ui(Ink::Chosen))
        }
    }

    /// What the effort chosen means on the highlighted model, which it names
    /// as itself ([`said`]) — `default`'s sentence is the one
    /// `crate::effort_panel::default_story` says.
    fn effort_note(&self, id: &str, effort: Option<Effort>) -> String {
        let name = &said(id);
        let Some(level) = effort else {
            return default_story(id, name, self.run);
        };
        match models::efforts(id) {
            Some([]) => {
                format!("{name} takes none — the setting is kept for the models that do")
            }
            Some(levels) if !levels.contains(&level) => {
                format!("{name} does not take {level} — it is sent none")
            }
            _ => "higher thinks longer, costs more; changing it re-caches".to_string(),
        }
    }

    /// A budget as the screen says it — `0` is the window of `model`.
    fn budget_word(&self, budget: u32, model: &str) -> String {
        match budget {
            0 => format!("model's {}", tokens(models::context_window(model))),
            n => tokens(n),
        }
    }

    /// The style of the cursor in `field`: `ui.chosen` while it is the field
    /// being changed, bold otherwise.
    fn focus_style(&self, field: Field, theme: &Theme) -> Style {
        if self.field == field {
            theme.ui(Ink::Chosen)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        }
    }

    /// A field's name, marked when the cursor is in it.
    fn label_span(&self, field: Field, name: &'static str, theme: &Theme) -> Span<'static> {
        let mark = if self.field == field { "▸" } else { " " };
        Span::styled(format!("{mark} {name}  "), self.focus_style(field, theme))
    }

    /// One choice in a field's row: `[chosen]`, plain, or dim where the
    /// highlighted model would not take it.
    fn option(
        &self,
        field: Field,
        here: bool,
        word: String,
        open: bool,
        theme: &Theme,
    ) -> Span<'static> {
        if here {
            Span::styled(format!("[{word}] "), self.focus_style(field, theme))
        } else if open {
            Span::raw(format!(" {word}  "))
        } else {
            Span::styled(format!(" {word}  "), theme.ui(Ink::TextQuiet))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn press(p: &mut Picker, keys: &str) -> Effect {
        let mut last = Effect::None;
        for c in keys.chars() {
            let code = match c {
                '\n' => KeyCode::Enter,
                '\t' => KeyCode::Tab,
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

    fn episode() -> EpisodeId {
        EpisodeId::from_seed(b"the session")
    }

    /// Enter's effect: `chosen`, for the session the screen opened on.
    fn applied(chosen: Chosen) -> Effect {
        Effect::Models {
            episode: episode(),
            chosen,
        }
    }

    fn now(model: &str) -> Chosen {
        Chosen {
            model: model.into(),
            effort: None,
            budget: 0,
        }
    }

    /// A picker with no run-wide effort, as a fresh config has.
    fn picker(now: Chosen, field: Field) -> Picker {
        Picker::new(episode(), now, None, field)
    }

    fn text(p: &Picker) -> String {
        let mut out = Vec::new();
        let legend = p.page(&mut out, &Theme::terminal());
        let mut lines: Vec<String> = out
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect();
        lines.push(legend.to_string());
        lines.join("\n")
    }

    /// **Enter on an untouched screen changes nothing** — and says so first.
    #[test]
    fn it_opens_on_what_is() {
        let mut p = picker(now("claude-opus-5-5"), Field::Model);
        assert!(text(&p).contains("Enter changes nothing."), "{}", text(&p));
        assert_eq!(press(&mut p, "\n"), applied(now("claude-opus-5-5")));
    }

    /// **The stub answering is said**, by name, and only when it does.
    #[test]
    fn the_stub_answering_is_said() {
        let p = picker(now("claude-opus-5-5"), Field::Model);
        assert!(!text(&p).contains("no API key"), "{}", text(&p));
        let p = p.stub_answering(true);
        assert!(
            text(&p).contains("no API key, so the stub answers, not opus"),
            "{}",
            text(&p)
        );
    }

    /// A model or a budget the list does not name is still what the session
    /// has: offered, the cursor on it, not silently swapped for the first —
    /// and a model at the head of its own route's group.
    #[test]
    fn what_the_list_does_not_name_is_offered() {
        let odd = Chosen {
            model: "claude-something-9".into(),
            effort: Some(Effort::Low),
            budget: 300_000,
        };
        let mut p = picker(odd.clone(), Field::Model);
        assert_eq!(p.chosen(), odd);
        assert_eq!(press(&mut p, "\n"), applied(odd));
        let cc = picker(now("claude-code/claude-opus-4-8"), Field::Model);
        let first_cc = cc.offers.iter().position(|o| via(&o.id) == "Claude Code");
        assert_eq!(
            first_cc.map(Row::Offer),
            Some(cc.cursor),
            "with its route's rows"
        );
        assert_eq!(
            text(&cc).matches("── Claude Code").count(),
            1,
            "one group, not two"
        );
    }

    /// The keys a user presses reach each field, and Enter says what changed
    /// — each model as itself, the line wrapped to the page.
    #[test]
    fn each_field_is_reached_and_changed() {
        let mut p = picker(now("claude-haiku-5-5"), Field::Model);
        press(&mut p, "j"); // haiku → sonnet
        assert_eq!(p.chosen().model, "claude-sonnet-5-5");
        press(&mut p, "\tllllll"); // effort: past max stays at max
        assert_eq!(p.chosen().effort, Some(Effort::Max));
        press(&mut p, "jl"); // context: the model's → 500k
        assert_eq!(p.chosen().budget, 500_000);
        let page = text(&p);
        assert!(
            page.contains(
                "Enter: model claude-haiku-5-5 → claude-sonnet-5-5, effort default → max,"
            ),
            "{page}"
        );
        assert!(page.contains("       context model's 1M → 500k"), "{page}");
        assert_eq!(
            press(&mut p, "\n"),
            applied(Chosen {
                model: "claude-sonnet-5-5".into(),
                effort: Some(Effort::Max),
                budget: 500_000,
            })
        );
    }

    /// `j` runs off the list into the fields, `k` back; `h/l` on the list is
    /// not a model change.
    #[test]
    fn the_fields_are_one_column() {
        let mut p = picker(now(models::STUB), Field::Model);
        let last = Row::Offer(p.offers.len() - 1);
        assert_eq!(p.cursor, last, "premise: the stub is the last row");
        press(&mut p, "j");
        assert_eq!(p.field, Field::Effort);
        press(&mut p, "jj");
        assert_eq!(p.field, Field::Context, "stops at the last field");
        press(&mut p, "kk");
        assert_eq!(p.field, Field::Model);
        assert_eq!(p.cursor, last, "and the model cursor did not move");
        press(&mut p, "hl");
        assert_eq!(p.chosen(), now(models::STUB));
    }

    /// Leaving is leaving, whatever was moved.
    #[test]
    fn esc_and_q_leave() {
        let mut p = picker(now("claude-opus-5-5"), Field::Effort);
        press(&mut p, "l");
        // A modified key is not its letter: `<C-q>` does not leave.
        let ctrl_q = p.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert_eq!(ctrl_q, Effect::None);
        assert_eq!(press(&mut p, "q"), Effect::Leave);
        let esc = p.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(esc, Effect::Leave);
    }

    /// **A model that takes no effort says so**, where the setting is chosen
    /// — the session's setting is kept, and that pane is sent none.
    #[test]
    fn a_model_without_effort_says_so() {
        let mut p = picker(now("claude-haiku-4-5-20251001"), Field::Effort);
        press(&mut p, "l");
        assert!(
            text(&p).contains("claude-haiku-4-5 takes none"),
            "{}",
            text(&p)
        );
        let mut p = picker(now("claude-opus-5-5"), Field::Effort);
        assert!(
            text(&p).contains("sends none: claude-opus-5-5 works at medium"),
            "{}",
            text(&p)
        );
        press(&mut p, "l");
        assert!(text(&p).contains("re-caches"), "{}", text(&p));
    }

    /// **`default` says what it follows**: the run's `[turn] effort` when it
    /// says one, in the note and the header.
    #[test]
    fn default_follows_the_runs_effort() {
        let p = Picker::new(
            episode(),
            now("claude-opus-5-5"),
            Some(Effort::High),
            Field::Effort,
        );
        let page = text(&p);
        assert!(page.contains("follows [turn] effort: high"), "{page}");
        assert!(page.contains("effort default (high)"), "{page}");
    }

    /// **Claude Code's rows are Claude Code's** — measured by their own
    /// model's row (a 1M window, not the unknown-model fallback), under their
    /// own rule, and with Claude Code's own default rather than an API one
    /// it does not run at.
    #[test]
    fn claude_codes_rows_are_its_own() {
        let p = picker(now("claude-code/opus"), Field::Model);
        assert_eq!(via(&p.chosen().model), "Claude Code");
        let page = text(&p);
        let rule = page.find("── Claude Code").expect("a rule over the group");
        let row = page
            .lines()
            .find(|l| l.contains('▸') && l.contains("opus"))
            .expect("the cursor is on a row");
        assert!(page.find(row).unwrap() > rule, "under its rule:\n{page}");
        assert!(row.contains("1M") && row.contains("your /effort"), "{row}");
        assert!(!row.contains("medium"), "not the API's default: {row}");
        let mut p = picker(now("claude-code/opus"), Field::Effort);
        press(&mut p, "h");
        assert!(text(&p).contains("Claude Code's own"), "{}", text(&p));
        assert!(
            offers().iter().all(|o| models::efforts(&o.id).is_some()),
            "every offer is known"
        );
        // Haiku and Fable by the id their short name means here, so what
        // `claude` runs is the model whose window the gauge reads — not
        // whatever Claude Code's own alias resolves to.
        for name in ["haiku", "fable"] {
            let row = offers()
                .into_iter()
                .find(|o| via(&o.id) == "Claude Code" && o.name == name)
                .unwrap();
            let pinned = format!("{}{}", claude_code::MODEL_PREFIX, models::resolve(name));
            assert_eq!(row.id, pinned, "{name}");
        }
    }

    /// **The header is one row at the box's width for every route**, and a
    /// change of route is said as one.
    #[test]
    fn the_header_fits_and_routes_are_said() {
        for offer in offers() {
            for effort in efforts() {
                let mut chosen = now(&offer.id);
                chosen.effort = effort;
                let p = Picker::new(episode(), chosen, Some(Effort::Xhigh), Field::Model);
                let mut out = Vec::new();
                p.page(&mut out, &Theme::terminal());
                let width: usize = out[0]
                    .spans
                    .iter()
                    .map(|s| crate::wrap::str_cells(&s.content))
                    .sum();
                assert!(width <= 74, "{width} cells: {}", out[0]);
            }
        }
        let mut p = picker(now("claude-opus-5-5"), Field::Model);
        for _ in 0..p.offers.len() {
            if p.chosen().model == "claude-code/opus" {
                break;
            }
            press(&mut p, "j");
        }
        assert!(
            text(&p).contains("model claude-opus-5-5 (API) → opus (Claude Code)"),
            "{}",
            text(&p)
        );
    }

    /// **No line of the page folds at the box's width** (74 cells inside):
    /// a folded line pushes the legend toward the bottom edge and splits a
    /// sentence across a column's indent. Every model offered, open on it —
    /// so its fold is open too — under a run level some do not take, and
    /// with every field changed, so the Enter line is its longest.
    #[test]
    fn no_line_of_the_page_folds() {
        let fits = |p: &Picker| {
            let mut out = Vec::new();
            p.page(&mut out, &Theme::terminal());
            for line in &out {
                let width: usize = line
                    .spans
                    .iter()
                    .map(|s| crate::wrap::str_cells(&s.content))
                    .sum();
                assert!(width <= PAGE, "{width} cells: {line}");
            }
        };
        for field in Field::ALL {
            for offer in offers() {
                let mut p = Picker::new(episode(), now(&offer.id), Some(Effort::Xhigh), field);
                fits(&p);
                // Everything changed: the far end of the list, a level, a budget.
                p.field = Field::Model;
                press(&mut p, "zRjjjjjjjjjjjjjjjjj\tll\tl");
                assert_ne!(p.chosen(), now(&offer.id), "premise: Enter changes it");
                fits(&p);
            }
        }
    }

    /// **The older models are a fold**, shut at first: one line saying what
    /// it holds, opened and shut by vim's `z` commands from anywhere, and
    /// from its line by Enter and `l`/`h`. Shut from inside, the cursor goes
    /// to its line; on its line no model is chosen (Steven, 10-31).
    #[test]
    fn the_older_models_are_a_fold() {
        let mut p = picker(now("claude-opus-5-5"), Field::Model);
        let page = text(&p);
        assert!(page.contains("    + older models (8)"), "{page}");
        assert!(!page.contains("opus-4-8"), "shut: {page}");
        press(&mut p, "jj"); // opus → fable → the fold's line
        assert_eq!(p.cursor, Row::Older);
        assert_eq!(p.chosen(), now("claude-opus-5-5"), "its line chooses none");
        assert!(
            text(&p).contains("Enter: open the older models"),
            "{}",
            text(&p)
        );
        assert_eq!(
            press(&mut p, "\n"),
            Effect::None,
            "Enter opens, applies nothing"
        );
        let page = text(&p);
        assert!(page.contains("  ▸ - older models (8)"), "{page}");
        assert!(page.contains("      opus-4-8 "), "under its line: {page}");
        press(&mut p, "j");
        assert_eq!(p.chosen().model, "claude-sonnet-5", "the first inside");
        press(&mut p, "h");
        assert_eq!(p.cursor, Row::Older, "shut from inside: to its line");
        assert!(!p.older_open);
        press(&mut p, "l");
        assert!(p.older_open, "`l` opens it");
        for (keys, open) in [("zc", false), ("za", true), ("zM", false), ("zR", true)] {
            press(&mut p, "k"); // off its line: a `z` reaches it anyway
            press(&mut p, keys);
            assert_eq!(p.older_open, open, "{keys}");
        }
        // A key after `z` is the chord's: `zq` leaves nothing.
        assert_eq!(press(&mut p, "zq"), Effect::None);
    }

    /// **A session on an older model opens on it** — the fold open, the
    /// cursor there: what is, is offered.
    #[test]
    fn an_older_session_opens_its_fold() {
        let p = picker(now("claude-opus-4-8"), Field::Model);
        assert!(p.older_open);
        let page = text(&p);
        // A row's cursor, not the field's label (`▸ Model`).
        let row = page
            .lines()
            .find(|l| l.starts_with("  ▸"))
            .expect("the cursor is on a row");
        assert!(row.contains("opus-4-8"), "{row}");
        assert!(page.contains("- older models (8)"), "{page}");
    }

    /// **The model is said as itself before Enter** — in the effort note and
    /// on the Enter line, by its id, so `sonnet-5` is known to be Sonnet 5,
    /// not 5.5, before it is chosen (Steven, 10-31: "we need some a priori").
    #[test]
    fn the_model_is_said_as_itself_before_enter() {
        let mut p = picker(now("claude-code/sonnet"), Field::Model);
        press(&mut p, "zR");
        // Bounded: a fold that did not open would hide it, and a hunt that
        // never ends fails no assertion.
        for _ in 0..p.offers.len() {
            if p.chosen().model == "claude-sonnet-5" {
                break;
            }
            press(&mut p, "k");
        }
        assert_eq!(
            p.chosen().model,
            "claude-sonnet-5",
            "reached, the fold open"
        );
        let page = text(&p);
        assert!(
            page.contains("Enter: model sonnet (Claude Code) → claude-sonnet-5 (API)"),
            "{page}"
        );
        assert!(
            page.contains("sends none: claude-sonnet-5 works at high"),
            "{page}"
        );
        // A dated id is said as the API's undated alias for it.
        assert_eq!(said("claude-haiku-4-5-20251001"), "claude-haiku-4-5");
        assert_eq!(said("claude-code/claude-fable-5-1"), "claude-fable-5-1");
        assert_eq!(said(models::STUB), "stub");
    }

    /// A budget above the highlighted model's window is said, in caution.
    #[test]
    fn a_budget_past_the_window_is_said() {
        let p = picker(
            Chosen {
                model: "claude-haiku-4-5-20251001".into(),
                effort: None,
                budget: 500_000,
            },
            Field::Context,
        );
        assert!(
            text(&p).contains("above haiku-4-5's 200k window"),
            "{}",
            text(&p)
        );
    }

    /// **The levels a model does not take are drawn dim** — asserted on the
    /// style the page carries, since a role nobody renders is no signal.
    #[test]
    fn levels_a_model_does_not_take_are_dim() {
        let theme = Theme::terminal();
        let style_of = |model: &str, word: &str| {
            let p = picker(now(model), Field::Model);
            let mut out = Vec::new();
            p.page(&mut out, &theme);
            out.iter()
                .flat_map(|l| l.spans.iter())
                .find(|s| s.content.trim() == word)
                .map(|s| s.style.fg)
                .unwrap_or_else(|| panic!("no `{word}` on the page"))
        };
        let dim = Some(theme.fg(Ink::TextQuiet));
        assert_eq!(style_of("claude-haiku-4-5-20251001", "max"), dim);
        assert_ne!(style_of("claude-opus-5-5", "max"), dim);
    }

    /// **A row's levels are a range only where they are one** — Opus 4.6
    /// skips `xhigh`, and `low–max` claimed it.
    #[test]
    fn a_row_says_the_levels_its_model_takes() {
        assert_eq!(effort_range("claude-opus-4-8"), "low–max · high");
        assert_eq!(
            effort_range("claude-opus-4-6"),
            "low/medium/high/max · high"
        );
        assert_eq!(effort_range("claude-haiku-4-5-20251001"), "—");
        assert_eq!(effort_range("claude-something-9"), "?");
    }

    #[test]
    fn tokens_read_as_a_column_says_them() {
        assert_eq!(tokens(1_000_000), "1M");
        assert_eq!(tokens(200_000), "200k");
        assert_eq!(tokens(4_096), "4k");
        assert_eq!(tokens(512), "512");
    }
}
