//! **Setup, wired** — what `crate::setup`'s answers do to the running app.
//!
//! Saving a key makes the session answer from the API at once, with no
//! relaunch: the backend is rebuilt from it ([`ApiFactory`]). Choosing Claude
//! Code asks `claude` whether it is logged in, never touching its credentials
//! (`claude_code::login_status`). Finishing from the menu switches this
//! session to the choice and writes it to `[tui] model`, so the next launch
//! starts there too (Steven, 10-04); finishing what a model switch opened
//! changes nothing else, because nobody asked for more.
//!
//! A child module of `app`, as `session_ops` and `config_file` are: it sees
//! `App`'s private fields, and the split is organisational.

use std::path::PathBuf;

use shannon_core::backend::claude_code::{LOGIN_CHECK_TIMEOUT, LoginStatus, login_status};

use super::config_file::tilde;
use super::*;
use crate::keys::KeyFrom;
use crate::setup::{Choice, Effect, Entry, Facts, Key, Setup};

impl App {
    /// **The setup menu** — `shannon setup`, `:setup`, and a first launch
    /// (`main`).
    pub fn open_setup(&mut self) {
        self.setup = Some(Setup::menu(self.setup_facts()));
        self.chat_dirty = true;
    }

    /// **Straight to what `choice` needs**, because a session was switched to
    /// a model that needs it — `why` says which; `status` is a login check
    /// already made.
    pub(crate) fn open_setup_at(
        &mut self,
        choice: Choice,
        why: String,
        status: Option<LoginStatus>,
    ) {
        self.setup = Some(Setup::at(choice, why, self.setup_facts(), status));
        self.chat_dirty = true;
    }

    /// **A model that is not set up opens setup on what it needs** (Steven,
    /// 10-04): an API model with no key, a Claude Code model whose `claude`
    /// is not ready. Right after the `:model` that asked — the user is looking
    /// at the answer — so it may take the screen.
    pub(crate) fn offer_setup_for(&mut self, resolved: &str) {
        match route(Some(resolved)) {
            Route::Configured if self.only_stub() => self.open_setup_at(
                Choice::Api,
                format!("`{resolved}` answers from the Anthropic API, which needs a key."),
                None,
            ),
            Route::ClaudeCode => {
                let status = self.claude_login();
                if status.ready() {
                    // Ready: what is left is whether it may run HERE.
                    self.ask_trust_up_front();
                } else {
                    self.open_setup_at(
                        Choice::ClaudeCode,
                        format!("`{resolved}` answers from Claude Code, which is not ready."),
                        Some(status),
                    );
                }
            }
            Route::Configured | Route::Stub => {}
        }
    }

    /// Setup, if it is open — for the tests: what a frame draws of it is
    /// asked of [`frame_draws`](Self::frame_draws), by the view and by sight
    /// alike.
    #[cfg(test)]
    pub(crate) fn setup(&self) -> Option<&Setup> {
        self.setup.as_ref()
    }

    /// **A screen the user asks for does not open over a y/N waiting**: the
    /// y/N takes the next key wherever you are (`KeyRoute::Confirm`), and
    /// setup takes every key first ([`App::setup_event`]), so the question
    /// would wait under a screen asking another. `true` when refused, and
    /// said as `verb`'s. Consumers, four: `:setup`, `:models` (a bare
    /// `:model` too), `:effort`, and the shelf (`:memories`, `:skills`).
    ///
    /// **A decision does not hold them** (Steven, 10-26: "I'd open it up so
    /// these don't block", on the condition that what a screen changes is
    /// reliable under it; it held them from 10-04 to 10-26, Reviewers and
    /// DR-A #11). A decision is its pane's, answered there, not by the next
    /// key; one waiting behind a screen is said on the legend or the bar
    /// (`setup::draw`, `setup::draw_panel`), as one arriving there always
    /// was. And nothing a screen sets reaches the turn waiting on it: the
    /// round after the answer asks under the config the turn began with
    /// (`Conversation::in_flight_config`;
    /// `a_tool_round_keeps_the_effort_its_turn_began_under`), and a model
    /// change for that session is refused while its turn waits, as `:model`
    /// refuses it (`refuse_if_busy`;
    /// `a_screen_over_a_waiting_call_changes_only_what_comes_after`).
    pub(crate) fn refuse_over_a_confirm(&mut self, verb: &str) -> bool {
        if self.pending_confirm.is_empty() {
            return false;
        }
        self.refuse(format!(
            "{verb}: answer the question waiting first — then `:{verb}`"
        ));
        true
    }

    /// **Setup's turn at an event**: `true` when it is open and took it. Keys
    /// and pastes are setup's while it is open — a stray `:` or a pasted key
    /// must never reach the prompt behind it; a resize still reaches the
    /// layout. `edit` opens a file the shelf screen asks to edit — `$EDITOR`
    /// from the event loop, which alone holds the terminal; a recorder in a
    /// test, which so drives the same path.
    pub(crate) fn setup_event(
        &mut self,
        event: &Event,
        edit: impl FnOnce(&Path) -> Result<()>,
    ) -> bool {
        let armed = self.prefix_in_flight();
        let Some(setup) = self.setup.as_mut() else {
            return false;
        };
        let effect = match event {
            // `Ctrl-L` redraws, here as everywhere: a screen that may be
            // garbled must still be repaintable.
            Event::Key(key) if matches!(classify(key), Shortcut::ForceRedraw) => return false,
            // **A prefix armed on the shelf owns the next key** — the rule
            // `viewer_claims_key` and the decision pane keep: `fx` disabled
            // the selected memory, its `x` taken as the shelf's verb, and
            // left `f` armed to eat the key after (review stop 16, A M3).
            Event::Key(key) if armed && setup.shelf_mut().is_some() => Effect::Navigate(*key),
            Event::Key(key) => setup.key(*key),
            Event::Paste(text) => setup.paste(text),
            Event::Resize(..) => return false,
            _ => Effect::None,
        };
        self.apply_setup(effect, edit);
        true
    }

    /// Test seam: [`setup_event`](Self::setup_event) with an editor that
    /// opens nothing — for a screen that never asks for one.
    #[cfg(test)]
    pub(crate) fn setup_event_for_test(&mut self, event: &Event) -> bool {
        self.setup_event(event, |_| Ok(()))
    }

    /// **Do what setup asked** — the one place its effects reach the app.
    /// `edit` is `setup_event`'s.
    pub(crate) fn apply_setup(&mut self, effect: Effect, edit: impl FnOnce(&Path) -> Result<()>) {
        self.apply_setup_effect(effect, edit);
        // The float's preview follows its cursor. What changes a file (an
        // edit, a removal, a new one) reads the lists again, which reads the
        // preview again too (`refresh_shelves`).
        self.refresh_shelf_preview(false);
        self.chat_dirty = true;
    }

    /// [`apply_setup`](Self::apply_setup)'s effects, one by one.
    fn apply_setup_effect(&mut self, effect: Effect, edit: impl FnOnce(&Path) -> Result<()>) {
        match effect {
            Effect::None => {}
            Effect::Leave => match self.setup.take().map(|s| s.entry()) {
                // The `:model` that opened it already switched: saying
                // "nothing changed" left the user on a model that cannot
                // answer, told otherwise (review 21, Reviewer C).
                Some(Entry::Model) => {
                    match self
                        .target_conversation()
                        .map(|c| c.episode().config.model.clone())
                    {
                        Some(model) => self.notify(format!(
                            "setup closed — this session is on `{model}`, which is not set up \
                             (`:setup` sets it up)"
                        )),
                        None => self.notify("setup closed — nothing changed"),
                    }
                }
                Some(Entry::Models) => self.notify("models closed — nothing changed"),
                Some(Entry::Effort) => self.notify("effort unchanged"),
                // A screen to look and edit from: closing it changes nothing
                // that wants saying.
                Some(Entry::Shelf) => {}
                _ => {
                    self.notify("setup closed — nothing changed (`:setup` opens it again)");
                    // Setup took launch's place: what this directory holds
                    // unread is said here, or never (review 2026-10-13, A).
                    self.say_unread_project();
                }
            },
            Effect::CheckClaudeCode => {
                let status = self.claude_login();
                if let Some(setup) = self.setup.as_mut() {
                    setup.set_status(status);
                }
            }
            Effect::UseKey(key) => {
                let used = self.use_api_key(key);
                self.finish_api_setup(used);
            }
            Effect::SaveKey(key) => {
                let saved = self.save_api_key(key);
                self.finish_api_setup(saved);
            }
            Effect::Finish(choice) => self.finish_setup(choice),
            Effect::Trust { dir, answer, held } => {
                self.setup = None;
                self.answer_trust(&dir, answer, held);
            }
            // Each applies to the session the screen opened on — and only
            // while it is still the one in front (`in_front`).
            Effect::Models { episode, chosen } => {
                self.setup = None;
                if let Some(buffer) = self.in_front(episode, "models") {
                    self.apply_models(buffer, chosen);
                }
            }
            Effect::Effort { episode, effort } => {
                self.setup = None;
                if let Some(buffer) = self.in_front(episode, "effort") {
                    self.switch_effort(buffer, effort);
                }
            }
            Effect::Edit(path) => self.edit_shelf_file(&path, edit),
            Effect::Toggle(toggle) => self.toggle_shelf_file(toggle),
            Effect::Remove(removal) => self.remove_shelf_file(removal),
            Effect::Create(create) => self.create_shelf_file(create, edit),
            Effect::AskModel(shelf) => self.ask_model_to_write(shelf),
            Effect::Navigate(key) => self.navigate_shelf(key),
        }
    }

    /// **Is `claude` ready to serve a turn?** Blocking, bounded by
    /// [`LOGIN_CHECK_TIMEOUT`]: asked only when the user just asked for it —
    /// setup's check, a `:model` switch — never in the background.
    pub(crate) fn claude_login(&self) -> LoginStatus {
        // In the environment a turn is given, or the login said is not the
        // one a turn runs under (`[claude_code] env = "minimal"`).
        login_status(
            &self.claude_program,
            &self.provider_env(),
            LOGIN_CHECK_TIMEOUT,
        )
    }

    /// Where setup saves a key: beside the config file the app was handed
    /// (`shannon_core::config::api_key_beside`, the rule launch reads by).
    fn api_key_path(&self) -> Option<PathBuf> {
        (self.user_config.as_deref().ok()).map(shannon_core::config::api_key_beside)
    }

    /// **The key the API answers with, and where it came from** — launch's
    /// (`main`), before setup can be opened. Masked from here on.
    pub fn set_key_in_use(&mut self, key: String, from: KeyFrom) {
        self.key_in_use = Some((Key::new(key), from));
    }

    /// What the setup screens state — gathered when setup opens.
    fn setup_facts(&self) -> Facts {
        let saved_at = self.api_key_path();
        Facts {
            key_path: saved_at.as_deref().map(tilde),
            // Launch's own rule (`keys::find_key`), not a copy of it: the copy
            // took an empty key-file variable as unset, which launch does not
            // (review 21, Reviewer C). Names only — never a value.
            key_overridden_by: crate::keys::read_ahead_of_saved(|var| std::env::var_os(var)),
            has_key: !self.only_stub(),
            key_from: self.key_in_use.as_ref().map(|(_, from)| match from {
                KeyFrom::File(path) => tilde(path),
                other => other.to_string(),
            }),
            key_is_saved: matches!(
                (&self.key_in_use, saved_at),
                (Some((_, KeyFrom::File(from))), Some(at)) if *from == at
            ),
        }
    }

    /// Setup's API path done: finished, or what went wrong said on its screen.
    fn finish_api_setup(&mut self, done: Result<(), String>) {
        match done {
            Ok(()) => self.finish_setup(Choice::Api),
            Err(why) => {
                if let Some(setup) = self.setup.as_mut() {
                    setup.say(why);
                }
            }
        }
    }

    /// **Answer from `key` now, writing nothing** — `None`: the key in use,
    /// which answers already.
    fn use_api_key(&mut self, key: Option<Key>) -> Result<(), String> {
        let Some(key) = key else {
            return Ok(());
        };
        self.backend = (self.make_api)(key.clone().expose(), self.config.cache)
            .map_err(|e| format!("the API client would not start ({e}) — nothing changed"))?;
        self.key_in_use = Some((key, KeyFrom::Pasted));
        Ok(())
    }

    /// **Save `key` owner-only, and answer from it** — `None`: the key in use
    /// (an `ANTHROPIC_API_KEY`, a pasted one), which answers already, so only
    /// the file is written.
    ///
    /// A pasted key's client is built first: a key the client cannot be built
    /// with is never written. The file is replaced rather than overwritten
    /// (`private_file::replace`) — owner-only whatever was there, and never
    /// half-written. The key itself goes nowhere else: not a log line, not a
    /// notice.
    fn save_api_key(&mut self, key: Option<Key>) -> Result<(), String> {
        let path = self
            .api_key_path()
            .ok_or("there is nowhere to save it — set HOME or XDG_CONFIG_HOME")?;
        let (key, backend) = match key {
            Some(key) => {
                let backend = (self.make_api)(key.clone().expose(), self.config.cache)
                    .map_err(|e| format!("the API client would not start ({e}) — nothing saved"))?;
                (key, Some(backend))
            }
            None => match &self.key_in_use {
                Some((key, _)) => (key.clone(), None),
                None => return Err("there is no key in use to save — paste one".into()),
            },
        };
        crate::private_file::replace(&path, &format!("{}\n", key.clone().expose()))
            .map_err(|e| format!("not saved — {e:#}"))?;
        info!(path = %path.display(), "API key saved by setup");
        if let Some(backend) = backend {
            self.backend = backend;
        }
        self.key_in_use = Some((key, KeyFrom::File(path)));
        Ok(())
    }

    /// **The choice is made and ready.** From the menu it becomes the
    /// default: written to `[tui] model`, and this session switches to it
    /// unless it already answers from there. From a model switch, the session
    /// already has the model it asked for.
    fn finish_setup(&mut self, choice: Choice) {
        let Some(setup) = self.setup.take() else {
            return;
        };
        if setup.entry() == Entry::Menu {
            match self.persist_provider(choice) {
                Ok(Some(saved)) => self.notify(saved),
                Ok(None) => {}
                Err(why) => self.refuse(why),
            }
            let buffer = self.prompt_target();
            let current = self
                .session_of(buffer)
                .map(|b| b.conversation.episode().config.model.clone())
                .unwrap_or_default();
            // **The session's model comes from the choice**, not from the
            // config: when `[tui] model` could not be written, the config
            // still names the OLD default, and the session switched to that
            // while the notice said the new choice answered (review 21,
            // Reviewer A). And a refused switch (a turn in flight) is said.
            if !answers_from(choice, &current) {
                let wanted = self.model_for(choice);
                if !self.switch_model(buffer, &wanted, &wanted) {
                    self.notify(format!(
                        "set up — but this session stays on `{current}` while its turn runs; \
                         `:model {wanted}` switches it after"
                    ));
                    return;
                }
            }
        }
        self.notify(match choice {
            Choice::Api => "set up: the Anthropic API answers",
            Choice::ClaudeCode => "set up: Claude Code answers — its tools pass Shannon's gate",
            Choice::Demo => "demo mode: the canned stub answers — `:setup` connects an agent",
        });
        // Set up is not yet allowed HERE: ask now, not at the first send.
        self.ask_trust_up_front();
    }

    /// **The model a session gets for `choice`**: the configured default when
    /// it already answers from there (`[tui] model = "claude-code/opus"`
    /// stays opus), else the path's own default — the API backend's model,
    /// Claude Code's `claude-code/sonnet`, the stub.
    fn model_for(&self, choice: Choice) -> String {
        let configured = self.default_model();
        if answers_from(choice, &configured) {
            return configured;
        }
        match choice {
            Choice::Api => self.backend.model().to_string(),
            Choice::ClaudeCode => shannon_core::backend::claude_code::DEFAULT_MODEL.to_string(),
            Choice::Demo => shannon_core::backend::models::STUB.to_string(),
        }
    }

    /// **Write the choice to `[tui] model`** in your config file, editing that
    /// key alone — comments and everything else as they were — and SET the
    /// running value to what the file now says, so new sessions start on it
    /// now too. The message says where it was saved, `None` when the file
    /// already said so.
    ///
    /// **That key alone, not the file re-read.** It took the file in as
    /// `:reload` does, and `:reload` is additive: the key the API choice
    /// REMOVES kept its running value, so a session moving from Claude Code
    /// back to the API switched straight back to `claude-code/sonnet`
    /// (Steven, 10-04). A layer holding the key says the file's value
    /// outright, empty included — and a file sourced later still wins over
    /// it. **Laid as your file's** (`Origin::User`), which it now is, not as
    /// a `:set`: typed, `:config save` wrote setup's choice into a project's
    /// file as if you had set it there (review 2026-10-13, A).
    fn persist_provider(&mut self, choice: Choice) -> Result<Option<String>, String> {
        let path = self.user_config().map_err(str::to_string)?;
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                return Err(format!(
                    "{} could not be read ({e}) — the choice holds for this run only",
                    tilde(&path)
                ));
            }
        };
        let edited = with_provider_model(&text, choice)?;
        if let Some(edited) = &edited {
            crate::private_file::overwrite(&path, edited).map_err(|e| {
                format!(
                    "{} NOT saved ({e:#}) — the choice holds for this run only",
                    tilde(&path)
                )
            })?;
        }
        let model = tui_model(edited.as_deref().unwrap_or(&text));
        let mut tui = toml::Table::new();
        tui.insert("model".into(), toml::Value::String(model));
        let mut table = toml::Table::new();
        table.insert("tui".into(), toml::Value::Table(tui));
        let layer = toml::to_string(&table).map_err(|e| {
            format!("[tui] model is saved but not in force ({e}) — it applies at launch")
        })?;
        let (config, notes) = self
            .config_stack
            .source(shannon_core::config::Origin::User, &path, &layer)
            .map_err(|why| {
                let why = shannon_core::Config::describe_parse_error(&why.0, &layer);
                format!("[tui] model is saved but not in force ({why}) — it applies at launch")
            })?;
        self.adopt_config(config);
        for note in notes {
            self.notify(note);
        }
        Ok(edited.map(|_| {
            format!(
                "saved to {} ([tui] model) — new sessions start there",
                tilde(&path)
            )
        }))
    }
}

/// What a config file's `[tui] model` says — empty when it says nothing,
/// which is the backend's default (`resolve_start_model`). A file that does
/// not parse says nothing here; [`with_provider_model`] has already refused it.
fn tui_model(text: &str) -> String {
    text.parse::<toml_edit::DocumentMut>()
        .ok()
        .and_then(|doc| {
            doc.get("tui")
                .and_then(|tui| tui.get("model"))
                .and_then(|model| model.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// Does a session on `model` already answer from `choice`?
fn answers_from(choice: Choice, model: &str) -> bool {
    matches!(
        (choice, route(Some(model))),
        (Choice::Api, Route::Configured)
            | (Choice::ClaudeCode, Route::ClaudeCode)
            | (Choice::Demo, Route::Stub)
    )
}

/// **`text` (a config file) with `[tui] model` saying `choice`** — `None`
/// when it already does. Edited in place (`toml_edit`), so comments and
/// formatting survive; only that key changes:
///
/// - Claude Code: `claude-code/sonnet`, unless it already names a Claude Code
///   model (`claude-code/opus` stays).
/// - Demo: `stub`.
/// - The API: the key comes OUT if it names Claude Code or the stub, so new
///   sessions take the API's default; an API model it names stays.
///
/// A file that does not parse is not touched, and the error names no line:
/// a config line can hold a secret (`[[alias]] value`).
pub(crate) fn with_provider_model(text: &str, choice: Choice) -> Result<Option<String>, String> {
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|_| {
        "your config does not parse — `:config edit` shows where; the choice holds for this \
         run only"
            .to_string()
    })?;
    let now = doc
        .get("tui")
        .and_then(|tui| tui.get("model"))
        .and_then(|model| model.as_str())
        .map(|model| route(Some(&shannon_core::backend::models::resolve(model))));
    // Indexing a `tui` that is not a table PANICS (`tui = "x"`, `[[tui]]`) —
    // which, from `:setup`, took the whole TUI down (review 21, Reviewer B).
    let set = |doc: &mut toml_edit::DocumentMut, model: &str| match doc.get("tui") {
        Some(tui) if !tui.is_table_like() => Err(
            "`tui` in your config is not a table — `:config edit` shows it; the choice holds for \
             this run only"
                .to_string(),
        ),
        _ => {
            doc["tui"]["model"] = toml_edit::value(model);
            Ok(())
        }
    };
    match (choice, now) {
        (Choice::ClaudeCode, Some(Route::ClaudeCode))
        | (Choice::Demo, Some(Route::Stub))
        | (Choice::Api, None | Some(Route::Configured)) => return Ok(None),
        (Choice::ClaudeCode, _) => {
            set(&mut doc, shannon_core::backend::claude_code::DEFAULT_MODEL)?
        }
        (Choice::Demo, _) => set(&mut doc, "stub")?,
        (Choice::Api, Some(Route::ClaudeCode | Route::Stub)) => {
            if let Some(tui) = doc.get_mut("tui").and_then(|t| t.as_table_like_mut()) {
                tui.remove("model");
            }
        }
    }
    Ok(Some(doc.to_string()))
}
