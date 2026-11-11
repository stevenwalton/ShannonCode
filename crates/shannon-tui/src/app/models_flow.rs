//! **`:models` and `:effort`, wired** — each choice applied through the one
//! door that owns it: `switch_model` for the session's model (its refusal
//! mid-turn, its setup offer), `switch_effort` for the session's effort, and
//! `:set` for the run's context budget (its layer, its echo). The screens
//! choose (`crate::model_picker`, `crate::effort_panel`); this applies, and
//! adds nothing a user could not have typed.
//!
//! **Effort is the session's, as its model is** (Steven, 10-04, after a
//! resumed session came back on its model and without its effort): kept in
//! the session's header, so it returns with the session. `[turn] effort` is
//! the default for a session that never chose one.
//!
//! A child module of `app`, as `setup_flow` is.

use super::*;
use crate::effort_panel::default_story;
use crate::model_picker::{Chosen, Field, Picker};
use crate::setup::Setup;
use shannon_core::backend::models;
use shannon_core::{Effort, EpisodeId};

impl App {
    /// `:models`, and a bare `:model` — the aimed session, the cursor on its
    /// model.
    pub(crate) fn models_command(&mut self) {
        if self.refuse_over_a_confirm("models") {
            return;
        }
        // A session's model: with none aimed at, nothing to choose for
        // (Steven, 11-11: "refuse").
        let Some(aimed) = self.aimed_session() else {
            let why = self.no_agent();
            return self.refuse(format!("`:models`: {why}"));
        };
        let episode = aimed.conversation.episode();
        let now = Chosen {
            model: episode.config.model.clone(),
            effort: episode.config.effort,
            budget: self.config.turn.max_context_tokens,
        };
        let stub = self.answering_model(&now.model) != now.model;
        let picker = Picker::new(episode.id, now, self.config.turn.effort, Field::Model)
            .stub_answering(stub)
            .labelled(&episode.label);
        self.setup = Some(Setup::models(picker));
        self.chat_dirty = true;
    }

    /// **`:effort [level]`** — this session's effort. Bare, the panel: vim's
    /// command-line window, one line per `:effort` you could type. With a
    /// level, or `default` to follow the run's `[turn] effort` again, exactly
    /// what that line would do.
    pub(crate) fn effort_command(&mut self, args: &str) {
        let buffer = self.prompt_target();
        let word = args.trim();
        if word.is_empty() {
            return self.open_effort_panel(buffer);
        }
        match Effort::parse_setting(word) {
            Ok(effort) => {
                self.switch_effort(buffer, effort);
            }
            Err(why) => self.refuse(format!("effort: {why}")),
        }
    }

    /// The panel, on `buffer`'s session and the model that answers it — not
    /// over a y/N waiting (`refuse_over_a_confirm`).
    fn open_effort_panel(&mut self, buffer: BufferId) {
        if self.refuse_over_a_confirm("effort") {
            return;
        }
        let Some(b) = self.session_of(buffer) else {
            let why = self.no_agent();
            return self.refuse(format!("effort: {why}"));
        };
        let episode = b.conversation.episode();
        let (id, now, label) = (episode.id, episode.config.effort, episode.label.clone());
        let panel = crate::effort_panel::Panel::new(
            id,
            self.session_model(buffer),
            now,
            self.config.turn.effort,
        )
        .labelled(&label);
        self.setup = Some(Setup::effort(panel));
        self.chat_dirty = true;
    }

    /// **The buffer in front, if it still holds `episode`** — the session a
    /// screen opened on. A composition can land while one is open (an `S`
    /// review, a prefill `:continue`) and put another session in front;
    /// applying to whatever is there would set a session the screen never
    /// named (Reviewers, 10-04).
    pub(super) fn in_front(&mut self, episode: EpisodeId, verb: &str) -> Option<BufferId> {
        let buffer = self.prompt_target();
        let here = self
            .session_of(buffer)
            .is_some_and(|b| b.conversation.episode().id == episode);
        if !here {
            self.refuse(format!(
                "{verb}: the session in front changed while it was open — nothing set; \
                 open it again"
            ));
        }
        here.then_some(buffer)
    }

    /// **Set `buffer`'s effort, write it down, and say what it reaches** —
    /// the one door for `:effort`, its panel and `:models`. `None` follows
    /// the run's `[turn] effort`.
    ///
    /// Unlike `:model`, not refused mid-turn: a turn keeps the config it began
    /// under (`Conversation::in_flight_config`), and a composition landing
    /// meanwhile takes the session's settings as they are when it lands — so
    /// this is the next turn's.
    pub(crate) fn switch_effort(&mut self, buffer: BufferId, effort: Option<Effort>) -> bool {
        let Some(target) = self.session_of_mut(buffer) else {
            let why = self.no_agent();
            self.refuse(format!("effort: {why}"));
            return false;
        };
        let ep = target.conversation.episode();
        let episode = ep.label.tag(ep.id);
        // The same choice again: said, and nothing written.
        if target.conversation.episode().config.effort == effort {
            let level = crate::effort_panel::word(effort);
            self.notify(format!("effort: {level} for {episode} — unchanged"));
            return true;
        }
        target.conversation.set_effort(effort);
        info!(effort = ?effort, episode = %episode, "session effort set");
        let written = self.write_preferences(buffer);
        let model = self.session_model(buffer);
        let name = models::short_name(&model).to_string();
        let run = self.config.turn.effort;
        let set = match effort {
            Some(level) => format!("effort: {level} for {episode}"),
            None => format!(
                "effort: default for {episode} — {}",
                default_story(&model, &name, run)
            ),
        };
        if let Err(e) = written {
            // A refusal, and the failure leads: it is the half the user finds
            // out about later otherwise.
            self.refuse(format!(
                "this run only — could not write it down ({e}): {set}"
            ));
            return true;
        }
        // **The caveat leads**, as `switch_model`'s does: the notice clips at
        // the row's width, and the half you must not miss is the half that
        // fits. Only for the session's OWN level: `default`'s sentence says
        // what it does itself.
        let stub_answers = self.only_stub() && model == models::STUB;
        let message = match (effort, self.effort_sent(buffer)) {
            (Some(level), None) if stub_answers => format!(
                "no API key, so the stub answers — {level} is kept for {episode}, sent once \
                 a key is set (`:setup`)"
            ),
            (Some(level), None) => format!(
                "{name} takes no effort — {level} is kept for {episode}, sent once it is on a \
                 model that does"
            ),
            _ => format!("{set}, from the next turn"),
        };
        self.notify(message);
        self.chat_dirty = true;
        true
    }

    /// The effort a turn on `buffer` would be sent now, if any — the seal's
    /// own answer (`sealing_config`), so the status line cannot show an effort
    /// the request will not carry.
    pub(crate) fn effort_sent(&self, buffer: BufferId) -> Option<Effort> {
        self.sealing_config(buffer).effort
    }

    /// **After a `:set` of `[turn] effort`, the default**: say so when the
    /// aimed session has its own, which the default does not reach — a
    /// `:set effort=low` that changed nothing visible would read as broken.
    pub(super) fn say_whose_effort(&mut self) {
        if let Some(own) =
            (self.aimed_session()).and_then(|b| b.conversation.episode().config.effort)
        {
            self.notify(format!(
                "this session keeps its own effort, {own} — `:effort default` follows [turn] effort"
            ));
        }
    }

    /// **Enter on `:models`**, for `buffer`'s session: each field that
    /// changed, through its door. The model first, so what the effort reaches
    /// is said of the model the session is now on; its switch may open setup
    /// for a model that is not set up yet, which no later notice disturbs.
    ///
    /// **All or nothing while a turn runs**: `:model` refuses then and
    /// `:effort` does not, so the screen's "Enter: model … effort …" applied
    /// half of itself (Reviewers, 10-04). A model change mid-turn refuses the
    /// whole Enter, and says why.
    pub(crate) fn apply_models(&mut self, buffer: BufferId, chosen: Chosen) {
        let Some(was) = self
            .session_of(buffer)
            .map(|b| b.conversation.episode().config.clone())
        else {
            let why = self.no_agent();
            return self.refuse(format!("models: {why}"));
        };
        let switch = was.model != chosen.model;
        let effort = was.effort != chosen.effort;
        let budget = chosen.budget != self.config.turn.max_context_tokens;
        if !switch && !effort && !budget {
            return self.notify("models: nothing changed");
        }
        if switch && self.refuse_if_busy("models", buffer) {
            return;
        }
        if switch {
            self.model_command(&chosen.model);
        }
        if effort {
            self.switch_effort(buffer, chosen.effort);
        }
        if budget {
            self.set_command(&format!("turn.max_context_tokens={}", chosen.budget), false);
        }
    }
}
