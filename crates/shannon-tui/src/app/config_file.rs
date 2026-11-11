//! **Your config, the way vim treats a vimrc**: `:config edit` SAVES it,
//! `:source {file}` applies a file over the running config, `:reload` sources
//! yours again, `:unsource {file}` takes a file back out, and `:scriptnames`
//! lists what is loaded. Built on `shannon_core::config::ConfigStack`, which
//! owns the rules; this is the half that talks to the user.
//!
//! **Editing does not apply** (Steven, 2026-09-27): swapping live state
//! mid-session "can cause breaking changes, especially when working in
//! memory", so the file is saved and CHECKED, and applying it is a separate,
//! deliberate verb — vim's `:so $MYVIMRC`, spelled `:reload`.
//!
//! A child module of `app` (like `session_ops`), so it sees `App`'s fields.

use std::path::PathBuf;

use super::*;
use shannon_core::config::{ConfigStack, Origin, SetReport, key_is_under, set};

/// **What `:config edit` starts a config that does not exist yet with** — a
/// pointer, not a copy of the defaults.
///
/// It used to seed the file with `DEFAULT_TOML`, every setting at its default.
/// That is vim's warning about `defaults.vim` (`:h defaults.vim`): a file
/// holding copies of the defaults "won't get updates when it changes", since
/// every copied line now OVERRIDES what a newer version ships. The file is a
/// sparse diff, so it starts empty but for the directions to fill it.
const SEED: &str = "\
# Your Shannon Code config: ONLY the settings you want to change.
#
# A key you leave out keeps its built-in default, so this file stays a short
# list of your choices, and a newer version's defaults still reach you.
# `:config dump` shows every setting with its default and what it does.
#
# Saving does not apply it: `:reload` does.
";

/// **What a config change does NOT apply until relaunch** — the settings read
/// once, at startup, into something that is never re-derived — and the word
/// `:help config` uses for each:
/// - `tui.mouse` → the terminal's capture mode (`TerminalGuard::enter`,
///   main.rs; `App::mouse_captured` records what it did)
/// - `[keymap]` → `Bindings::with_overrides` (main.rs)
/// - `[[alias]]`, `[secrets] env`, `alias_home` → the substitution table
///   (`App::new`; rebuilding it would re-mint every handle in the transcript)
/// - `[permissions] scratch` → `App::scratch`, vouched for once
///   (`scratch::prepare`) and handed to Claude Code's backend at launch
///
/// Launch-time settings ([`LAUNCH_KEYS`]) are left off: they describe a
/// launch, so "on relaunch" is what they mean. Applied at once by
/// `adopt_config`: `tui.prompt.height` (resizes the prompt, and only the
/// prompt) and `[cache]` (handed to the backend). `tui.model` is read live
/// — it is the model the NEXT new session starts on.
///
/// Keyed, so a change names the ones IT touched rather than reciting the
/// whole list after every `:reload`. The manual's sentence is held to this
/// table by `the_relaunch_list_is_the_manuals`.
const RELAUNCH_KEYS: &[(&str, &str)] = &[
    ("tui.mouse", "mouse"),
    ("keymap", "keymaps"),
    ("alias", "aliases"),
    ("secrets.alias_home", "aliases"),
    ("secrets.env", "[secrets] env"),
    ("permissions.scratch", "scratch"),
];

/// **The settings that describe a launch** — read once, when Shannon starts,
/// and meaningless after — so a file that changes one is right to stay quiet
/// (it will apply next time), and a `:set` of one is not: it does nothing
/// now and is gone when you quit.
const LAUNCH_KEYS: &[&str] = &["tui.resume", "tui.prompt.start_mode"];

/// Why there is no config file of yours: nothing to find it by.
pub(super) const NO_USER_CONFIG: &str = "no config path — set HOME or XDG_CONFIG_HOME";

impl App {
    /// Hand over the files launch built the running config from, and which of
    /// them is yours (`main`, from `Config::config_path`) — and so where theme
    /// and program-model files are: the theme `tui.theme` names is loaded
    /// here, and the user's models, which the rules are then read by.
    pub fn set_config_stack(&mut self, stack: ConfigStack, user_config: Option<PathBuf>) {
        self.adopt_stack(stack, user_config.ok_or(NO_USER_CONFIG));
    }

    /// [`set_config_stack`](Self::set_config_stack) for a launch that reads
    /// no file of yours — the tutor's: `why` is what `:reload` and `:config
    /// edit` say, there being nothing to read or open.
    pub fn set_config_stack_without_yours(&mut self, stack: ConfigStack, why: &'static str) {
        self.adopt_stack(stack, Err(why));
    }

    fn adopt_stack(&mut self, stack: ConfigStack, user_config: Result<PathBuf, &'static str>) {
        self.config_stack = stack;
        self.user_config = user_config;
        self.load_theme();
        let _ = self.load_models();
        let _ = self.rebuild_rules();
        self.keep_told();
    }

    /// **What `[exec] env` says** — a name left out, a `PATH` entry a file
    /// could run from as a name your rules allow (`exec::configured_env`).
    /// At launch (`main`) and on a change to it.
    pub fn env_notes(&self) -> Vec<String> {
        self.exec_env().1
    }

    /// **`[exec] env`, as commands are given it, and what it says** — the
    /// one reading for its three readers (a command's [`exec_config`], a
    /// request's `env` at `spawn_tagged`, the notes above), so none can hand
    /// on a variable the others hold back: Shannon's own, one a secret was
    /// lent from (`Aliases::lent_vars`, review stop 14, Reviewer E F1), or
    /// one `[exec] withhold` names.
    ///
    /// [`exec_config`]: App::exec_config
    pub(crate) fn exec_env(&self) -> (Vec<(String, String)>, Vec<String>) {
        shannon_core::exec::configured_env(
            &self.config.exec.env,
            &self.aliases.lent_vars(),
            &self.config.exec.withhold,
        )
    }

    /// **What no command is handed, whatever names it** — every variable a
    /// secret was lent from, every one of Shannon's own holding a held value
    /// however it came to be held ([`held_vars`](Self::held_vars)), and
    /// `[exec] withhold` (Steven, 10-31: "define variables Shannon gets or
    /// doesn't"). Its readers: a command's `exec_config`, `claude`'s
    /// ([`provider_env`](Self::provider_env)), and the gate, which reads them
    /// as unset (`rebuild_rules`).
    pub(crate) fn withheld(&self) -> Vec<String> {
        let mut names = self.aliases.lent_vars();
        let held = self.held_vars().into_iter().map(|(name, _)| name);
        for name in held.chain(self.config.exec.withhold.iter().cloned()) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }

    /// **Shannon's own variables whose value is held as a secret**, with its
    /// handle (`Aliases::held_vars`): a token pasted into Shannon while the
    /// shell exports it stayed in `claude`'s environment, read by `echo
    /// $NAME` unasked (the close of review stop 14, V 3). The table changes
    /// at a mark; the environment does not, so both readers ask again.
    pub(crate) fn held_vars(&self) -> Vec<(String, String)> {
        self.aliases.held_vars(self.process_env.iter().cloned())
    }

    /// **What `claude` is given of this environment** — one value for both
    /// of its spawns, a turn (`spawn_tagged`) and the login check
    /// (`claude_login`), read live so `:set` applies from the next.
    pub(crate) fn provider_env(&self) -> shannon_core::ProviderEnv {
        shannon_core::ProviderEnv {
            minimal: self.config.claude_code.env == shannon_core::config::EnvPolicy::Minimal,
            set: self.exec_env().0,
            withheld: self.withheld(),
        }
    }

    /// **What `[tui.graph_glyphs]` holds that is not a mark**, each with why
    /// — what [`NodeGlyphs::read_override`](crate::graph_view::NodeGlyphs::read_override),
    /// the rule the graph applies, refuses. At launch (`main`) and on a change
    /// to it: it was a log line alone, and the mark simply stayed the default.
    pub fn glyph_notes(&self) -> Vec<String> {
        self.config
            .tui
            .graph_glyphs
            .iter()
            .filter_map(|(role, value)| {
                crate::graph_view::NodeGlyphs::read_override(role, value).err()
            })
            .collect()
    }

    /// **Where Shannon started, when that puts homes inside the project** —
    /// at launch (`main`), from `start_note` over this run's start and
    /// `$HOME`, each as the filesystem resolves it where it can.
    pub fn start_notes(&self) -> Vec<String> {
        let resolved = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let (Some(start), Some(home)) = (self.start.as_deref(), shannon_core::secrets::home_dir())
        else {
            return Vec::new();
        };
        start_note(&resolved(start), &resolved(Path::new(home)))
            .into_iter()
            .collect()
    }

    /// Your config file, or why there is none.
    pub(super) fn user_config(&self) -> Result<PathBuf, &'static str> {
        self.user_config.clone()
    }

    /// `:config edit` — your config file in `$EDITOR`, saved and checked on
    /// the way out, **not applied**. A file that does not exist yet starts
    /// from [`SEED`], so this is also "create my config".
    ///
    /// **The file is written owner-only** (`0600`, dir `0700`), matching the
    /// episode store and the log dir. It is not merely preference data:
    /// `secrets.extra` is documented as the place to put "a database password,
    /// an internal token" verbatim, so `:config edit` — the same command that
    /// *creates* the file — must not leave it mode `0644`.
    pub(super) fn edit_config_file(&mut self, terminal: &mut ShannonTerminal) {
        let path = match self.user_config() {
            Ok(path) => path,
            Err(why) => return self.refuse(why),
        };
        let initial = match editable_config(&path) {
            Ok(text) => text,
            Err(why) => return self.refuse(format!("{} NOT opened — {why}", path.display())),
        };
        let edited = match crate::editor::edit_text(
            terminal,
            self.mouse_captured,
            &initial,
            crate::editor::Filetype::Toml,
        ) {
            Ok(text) => text,
            Err(e) => {
                warn!(error = ?e, ":config edit failed");
                return self.refuse(format!("config not saved — the editor failed: {e}"));
            }
        };
        self.save_config(&path, &edited);
    }

    /// **Save an edited config and say whether `:reload` would take it** —
    /// the running config is untouched either way.
    ///
    /// The check is the file on its own, through the parse launch uses: a
    /// typo, an unknown key, a value that would be clamped, all reported now,
    /// while the file is in front of you, rather than at the next `:reload`
    /// or launch. Split from [`edit_config_file`](Self::edit_config_file) so
    /// it is testable without a `$EDITOR` subprocess.
    pub(super) fn save_config(&mut self, path: &Path, edited: &str) {
        if let Err(e) = crate::private_file::overwrite(path, edited) {
            warn!(path = %path.display(), error = %e, "could not write config");
            return self.refuse(format!("config NOT saved: {e}"));
        }
        match shannon_core::Config::parse_ready(edited) {
            Ok((_, notes)) => {
                info!(path = %path.display(), "config saved");
                self.notify("config saved — `:reload` applies it");
                for note in notes {
                    self.notify(note);
                }
            }
            Err(e) => {
                // **The line number, never the line.** The rendered error quotes
                // the offending line, which can be a secret — an `[[alias]]
                // value` — and this logged it whole (`error = %e`), and showed
                // it in a notice, which is logged too (`App::post`).
                let said = shannon_core::Config::describe_parse_error(&e, edited);
                warn!(path = %path.display(), error = %said, "saved config does not parse");
                self.refuse(format!(
                    "config saved, but it does not parse ({said}) — `:reload` would refuse it"
                ));
            }
        }
    }

    /// `:source {file}` — apply a config file over the running one.
    ///
    /// Bare `:source` is REFUSED rather than guessed at: vim's bare form
    /// sources the current buffer, which there is no equivalent of here, and
    /// reading it as "my config" would take the meaning vim gives it (Steven:
    /// `:reload` is the reload; a bare `:source` is an error).
    pub(super) fn source_command(&mut self, args: &str) {
        let arg = args.trim();
        if arg.is_empty() {
            return self.refuse("`:source` needs a file — `:reload` sources your config again");
        }
        let path = config_path_arg(arg);
        // **`:source` of your own config IS `:reload`** — one verb, not two
        // behaviours chosen by which one you typed: as `Sourced` it earned the
        // safety note naming your own file, and `:scriptnames` listed it
        // twice, as `user` and as `sourced` (Reviewer A). And so it reads
        // the project's file after yours, as `:reload` does. The project's
        // loaded file, likewise, is read again AS the project's: as `Sourced`
        // it stood beside its own layer, a second copy over yours.
        let same = |file: Option<&Path>| {
            file.is_some_and(|file| shannon_core::config::same_file(file, &path))
        };
        // The project's file where trusted, loaded or not: after an
        // `:unsource` it came back as `Sourced`, and `:reload` then laid a
        // `Project` copy beside it (review 2026-10-13, A).
        let project = self
            .start
            .as_deref()
            .and_then(|start| self.trusted.project_file(start));
        if same(self.user_config.as_deref().ok()) {
            self.reload_command();
        } else if same(project.as_deref()) {
            self.source_file(Origin::Project, &path);
        } else {
            self.source_file(Origin::Sourced, &path);
        }
    }

    /// `:reload` — source your config again, and then the project's when one
    /// is loaded (6d), the order launch reads them in (Steven, 10-13:
    /// system, global, project) — so an edit to either is taken. The
    /// project's stays over yours however yours arrives (the stack's
    /// placement, `ConfigStack::source_all`). **Additive**, as `:so
    /// $MYVIMRC` is: a key you deleted from a file keeps its running value
    /// until relaunch (or `:unsource` of the file).
    ///
    /// **A file not there is left out; one loaded and gone since is taken
    /// off, and said** — as launch, which reads what is there. A missing
    /// file refused the whole reload: a deleted project file kept your own
    /// edit out until `:unsource`, and with no config of yours (a normal
    /// state) a project edit was never read again (review 2026-10-13, A,
    /// C).
    pub(super) fn reload_command(&mut self) {
        let mine = self.user_config();
        // The project's: the one loaded, or one there now in a trusted
        // start (written by hand, or by `:config! save` before a `:trust`).
        let project = self
            .config_stack
            .project_file()
            .map(Path::to_path_buf)
            .or_else(|| {
                let start = self.start.as_deref()?;
                self.trusted.project_file(start)
            });
        let mut files: Vec<(Origin, PathBuf)> = Vec::new();
        let mut gone = false;
        let wanted = [
            mine.as_ref().ok().map(|p| (Origin::User, p.clone())),
            project.map(|p| (Origin::Project, p)),
        ];
        for (origin, path) in wanted.into_iter().flatten() {
            if path.is_file() {
                files.push((origin, path));
            } else if let Some((config, notes)) = self.config_stack.unload(&path) {
                gone = true;
                let changed = self.adopt_config(config);
                self.report_change(&format!("{} is gone — taken off", tilde(&path)), &changed);
                for note in notes {
                    self.notify(note);
                }
            }
        }
        match (files.is_empty(), gone, mine) {
            (false, ..) => self.source_files(&files),
            (true, true, _) => {}
            (true, false, Err(why)) => self.refuse(why),
            (true, false, Ok(path)) => self.refuse(format!(
                "{} NOT sourced — no such file; `:config edit` makes it",
                path.display()
            )),
        }
    }

    /// `:unsource {file}` — take every layer a file added back out, so its
    /// settings fall back to what the rest of the stack says.
    pub(super) fn unsource_command(&mut self, args: &str) {
        let arg = args.trim();
        if arg.is_empty() {
            return self.refuse("`:unsource` needs a file — `:scriptnames` lists what is loaded");
        }
        let path = config_path_arg(arg);
        match self.config_stack.unload(&path) {
            Some((config, notes)) => {
                let changed = self.adopt_config(config);
                self.report_change(&format!("unsourced {}", path.display()), &changed);
                for note in notes {
                    self.notify(note);
                }
            }
            None => self.refuse(format!(
                "{} is not loaded — `:scriptnames` lists what is",
                path.display()
            )),
        }
    }

    /// **`:scriptnames`' page** — the config files in effect, in the order
    /// they apply, one a line and numbered as vim numbers its list of sourced
    /// scripts, each with where it came from, and naming its file
    /// (`Row::Script`). Read live by its pane (`listing_lines`), so a
    /// `:reload` or `:unsource` shows at once.
    pub(super) fn scripts_lines(&self) -> crate::page::Lines {
        let mut out = crate::page::Lines::default();
        for (i, (origin, path)) in self.config_stack.layers().enumerate() {
            out.row(
                &crate::page::Row::Script(path.to_path_buf()),
                &format!("{:>3}: {:<7} {}", i + 1, origin.word(), path.display()),
            );
        }
        if out.lines().is_empty() {
            out.text("no config files loaded — running on built-in defaults");
        }
        out
    }

    /// **Read `path` and put it on the stack** — one file of
    /// [`source_files`](Self::source_files).
    pub(super) fn source_file(&mut self, origin: Origin, path: &Path) {
        self.source_files(&[(origin, path.to_path_buf())]);
    }

    /// **Read `files` and put them on the stack, in order, as one change** —
    /// the core `:source` and `:reload` share. Nothing changes unless every
    /// file reads AND fits: a reload that took yours and refused the
    /// project's would leave yours over it.
    pub(super) fn source_files(&mut self, files: &[(Origin, PathBuf)]) {
        let mut texts = Vec::with_capacity(files.len());
        for (_, path) in files {
            match shannon_core::config::read_file(path) {
                Ok(Some(text)) => texts.push(text),
                Ok(None) => {
                    return self.refuse(format!(
                        "{} NOT sourced — no such file; nothing changed",
                        path.display()
                    ));
                }
                Err(why) => {
                    return self.refuse(format!(
                        "{} NOT sourced — {why}; nothing changed",
                        path.display()
                    ));
                }
            }
        }
        let read: Vec<(Origin, &Path, &str)> = files
            .iter()
            .zip(&texts)
            .map(|((origin, path), text)| (*origin, path.as_path(), text.as_str()))
            .collect();
        match self.config_stack.source_all(&read) {
            Ok((config, notes)) => {
                let changed = self.adopt_config(config);
                let names: Vec<String> =
                    files.iter().map(|(_, p)| p.display().to_string()).collect();
                self.report_change(&format!("sourced {}", names.join(", then ")), &changed);
                for note in notes {
                    self.notify(note);
                }
            }
            Err((i, e)) => {
                // The line number, never the line — see `save_config`.
                let (path, text) = (&files[i].1, &texts[i]);
                let said = shannon_core::Config::describe_parse_error(&e.0, text);
                warn!(path = %path.display(), error = %said, "config not sourced");
                self.refuse(format!(
                    "{} NOT sourced — {said}; nothing changed",
                    path.display()
                ));
            }
        }
    }

    /// **Make `config` the running config** — the one door every config
    /// change walks through: `:source`, `:reload`, `:unsource`, `:set`, and
    /// setup's `[tui] model` (`app::setup_flow::persist_provider`).
    ///
    /// Assigning the config is not applying it. Everything derived from it is
    /// stale until re-derived, and the permission engine is the one that
    /// matters ([`rebuild_rules`](Self::rebuild_rules)).
    ///
    /// **`self.aliases` is NOT re-derived**, and the message must not pretend
    /// otherwise. It is built once at construction from `[[alias]]`,
    /// `[secrets] env` and `alias_home`, and rebuilding it cannot be the fix:
    /// `Aliases::seed` is per-table session entropy, so a fresh table mints
    /// DIFFERENT handles and every handle already in the transcript becomes
    /// one the gate correctly refuses as injected. It wants a merge into the
    /// existing table that preserves `"marked by you"` entries — docketed,
    /// because what a *removed* alias should do is a decision
    /// ([[project_shannon_message_honesty]]).
    ///
    /// Returns the settings that changed; saying so is the caller's
    /// ([`report_change`](Self::report_change) for a file; `:set` echoes the
    /// values it set). Not logged here: the stack logs each layer with the
    /// keys it changed, and a notice is logged by `post`.
    pub(super) fn adopt_config(&mut self, config: Config) -> Vec<String> {
        let mut changed = self.config.changed_keys(&config);
        let fold_was = self.config.tui.fold;
        self.config = Arc::new(config);
        self.keep_told();
        // Where the model writes is vouched for when it is set, BEFORE the
        // one rebuild below: a `:set memory=off` takes the write rule away
        // now, and what the rebuild does to the open decision is said, never
        // done by a second rebuild out of sight (`vouch_memory_write`).
        let knowledge = changed
            .iter()
            .any(|k| key_is_under(k, "memory") || key_is_under(k, "skills"));
        // `{project}` named again when how it is named changes, and the
        // project's root found again when what marks it does — before the
        // vouch, so the write place vouched is the new one. Only then: they
        // are the launch's otherwise (`Roots::follow`, which launch runs).
        if changed
            .iter()
            .any(|k| k == "memory.project" || k == "memory.project_root")
            && let Some(roots) = self.knowledge_roots.as_mut()
        {
            roots.follow(&self.config.memory);
        }
        let notes = match knowledge {
            true => self.vouch_memory_write(),
            false => Vec::new(),
        };
        // The models first, every change: a `:reload` after editing a model
        // FILE changes no setting, and the rules below are read by them. A
        // file that reads differently is a change, and named as one.
        changed.extend(self.load_models());
        // A rule that arrives by file answers the open decision as `:allow`
        // would; say what happened to it.
        if let Some(super::Revisit::Changed(outcome)) = self.rebuild_rules() {
            self.notify(format!("the open decision: {outcome}"));
        }
        for note in notes {
            self.notify(note);
        }
        // What `[exec] env` leaves out, or holds that runs as your rules'
        // names, said when it changes (and at launch, `env_notes`).
        if changed.iter().any(|k| key_is_under(k, "exec.env")) {
            for note in self.env_notes() {
                self.notify(note);
            }
        }
        // The prompt's declared height is layout, not a value anything reads
        // per frame — so a change to it is APPLIED here, or it waited for the
        // next `<C-w>=` while the notice said it had changed (Steven, 50 rows).
        // Every tab's: a parked one came back with the old (r12-A M-2).
        if changed.iter().any(|k| k == "tui.prompt.height")
            && let Some((w, h)) = self.frame
        {
            let height = self.config.tui.prompt.height;
            for workspace in self.workspaces_mut() {
                workspace.set_prompt_height(height, w, h);
            }
        }
        // `[cache]` is decided in ONE place, the backend, which was built with
        // the launch policy — so `:set nocache.enabled` echoed success while
        // every request kept caching (Reviewers A and C). Hand it the change.
        if changed.iter().any(|k| key_is_under(k, "cache")) {
            self.backend.set_cache(self.config.cache);
        }
        // `[secrets] password_protect` changes only in a file (`:set` refuses
        // it, `config::set`). Sealing is not built yet (the plan's P5 seals or
        // unseals here): said, so the file's change is not taken as done —
        // and nothing is moved.
        if changed.iter().any(|k| k == "secrets.password_protect") {
            self.notify(
                "secrets.password_protect is not built yet — your kept secrets stay as they are, \
                 and nothing asks for a passphrase",
            );
        }
        // An open tree is BUILT with `[tui] graph_*` — the tree style's
        // labels, the head glyphs, the order — so a change to one rebuilds
        // it. It did not: `:set tui.graph_glyphs.child=…` echoed the new
        // value while the tree kept the old one until something else (a
        // background turn, an operation) happened to rebuild it.
        if changed.iter().any(|k| k.starts_with("tui.graph_")) {
            self.refresh_graph_views();
        }
        if changed.iter().any(|k| key_is_under(k, "tui.graph_glyphs")) {
            for note in self.glyph_notes() {
                self.notify(note);
            }
        }
        // `[tui.fold]` decides how a window's blocks start: the drawing asks
        // it each frame (`App::fold_rule`), the keys only through the copy
        // they move in, made at a sync — so a change is a sync owed, or `j`
        // stepped by the old rule under a thought drawn by the new (review
        // stop 15, B H2). The focused window syncs on the next frame,
        // another when it is focused. And a kind whose rule changed has its
        // folds put back to it, as vim's `:set foldlevel` does (E L5) — that
        // kind's only.
        let ruled = crate::output::ruled_otherwise(fold_was, self.config.tui.fold);
        self.folds_to_their_rule(&ruled);
        // Every change, not only one to `tui.theme`: a `:reload` after
        // editing the theme FILE changes no setting, and is how you would
        // expect to see it. A theme that did not change says nothing.
        self.load_theme();
        changed
    }

    /// **Say what loading or unloading a file changed** — the settings that
    /// CHANGED, since a sourced file is often written by someone else (a
    /// project, a model) and "sourced" alone would hide what it did; and, of
    /// those, the ones only a relaunch applies.
    fn report_change(&mut self, what: &str, changed: &[String]) {
        if changed.is_empty() {
            return self.notify(format!("{what} — nothing changed"));
        }
        self.notify(format!("{what} — changed: {}", changed.join(", ")));
        if let Some(later) = relaunch_note(changed) {
            self.notify(later);
        }
    }

    /// **`:config save`** — what this session set, into the project's
    /// `.shannon/config.toml` (Steven, 10-13: "saves the local settings
    /// created in that session … unsurprising to any users"): each option a
    /// `:set` named, at its value now (`ConfigStack::typed_for_project`),
    /// and the rules taught here — an `:allow`/`:ask`/`:deny` added to its
    /// rules, a shipped rule `:un…`'d added to its `remove`, one of its own
    /// `:un…`'d taken out. Edited in place, comments kept ([`save_into`]);
    /// made, with `.shannon/`, if there is none.
    ///
    /// Where the directory is trusted it is read back at once, and its rules
    /// come from it now: `:rules` lists them once, under the project. Not
    /// trusted, it would not be read, so it is refused — unless `!` (Steven:
    /// "I want `:config! save` to write even if we haven't trusted").
    ///
    /// **`:config save <file>`** writes the same into the file you name
    /// (Steven, 11-10: the README's "export your settings"), trusted or not,
    /// and reads nothing back: what it holds is in force here already, and
    /// `:source` takes it elsewhere. Only what is added — a `-=` and a rule
    /// `:un…`'d out of the project's own entries are that file's to lose,
    /// said and not written. A link is refused, as the project's is: the
    /// file is replaced, and a dotfile's link would be lost; its target can
    /// be named. **The project's own file by name is the bare form**, trust
    /// gate, link checks and read-back with it — the same file however it is
    /// spelt (`same_file`: `../here/…`, `/tmp` for `/private/tmp`, a case the
    /// volume ignores). Compared as spelt, those wrote it past the gate
    /// (review stop 17, V3 M1).
    pub(super) fn config_save(&mut self, bang: bool, to: Option<&str>) {
        use shannon_core::permission::Origin as From;
        let Some(start) = self.start.clone() else {
            return self.refuse("which directory is this? Shannon cannot tell");
        };
        let project = shannon_core::config::project_path(&start);
        let named = to
            .map(|to| start.join(config_path_arg(to)))
            .filter(|path| !shannon_core::config::same_file(path, &project));
        let trusted = self.trusted.covers(&start);
        if named.is_none() && !trusted && !bang {
            return self.refuse(
                "this directory is not trusted, so its .shannon/config.toml would not be read — \
                 `:trust .` first, or `:config! save` writes it anyway",
            );
        }
        // The repository holds `.shannon/` and what is in it: a link at
        // either carries the save out of it. One at the FILE was read
        // through and replaced by its target's whole text — your own
        // config's, `[[alias]]` values and all, copied into the repository
        // (review 2026-10-13, A). The file is replaced, never written
        // through (`private_file::replace`).
        let dir = start.join(shannon_core::config::PROJECT_DIR);
        let path = named.clone().unwrap_or(project);
        let links: &[&Path] = match named {
            Some(_) => &[&path],
            None => &[&dir, &path],
        };
        for at in links {
            if std::fs::symlink_metadata(at).is_ok_and(|m| m.file_type().is_symlink()) {
                return self.refuse(match named {
                    Some(_) => format!(
                        "{} is a link — saving replaces the file, so name the one it points to",
                        tilde(at)
                    ),
                    None => format!(
                        "{} is a link — Shannon saves only into the project's own \
                         .shannon/config.toml",
                        tilde(at)
                    ),
                });
            }
        }
        let typed = self.config_stack.typed_for_project(&self.config);
        let values = typed.values;
        let (mut lists, mut left) = self.saved_rules(typed.lists, typed.left);
        if named.is_some() {
            for list in &mut lists {
                for taken in list.take.drain(..) {
                    left.push(format!(
                        "`{taken}` out of {} — the project's own entry, which `:config save` \
                         with no file takes out",
                        list.path
                    ));
                }
            }
            lists.retain(|list| !list.add.is_empty());
        }
        let said: Vec<String> = std::iter::once((!values.is_empty()).then(|| {
            let keys = shannon_core::config::set::leaf_paths(&values);
            format!("{} ({})", count(keys.len(), "option"), keys.join(", "))
        }))
        .chain(lists.iter().map(|l| Some(said_of(l))))
        .flatten()
        .collect();
        if said.is_empty() {
            let mut why = "nothing to save — no `:set`, `:allow`, `:ask`, `:deny` or `:un…` \
                           this session"
                .to_string();
            if !left.is_empty() {
                let file = match named {
                    Some(_) => "a file of your naming",
                    None => "a project's file",
                };
                why = format!("{why} that {file} can hold: {}", left.join("; "));
            }
            return self.refuse(why);
        }
        let text = match shannon_core::config::read_file(&path) {
            Ok(text) => text.unwrap_or_default(),
            Err(why) => return self.refuse(format!("{} NOT saved — {why}", tilde(&path))),
        };
        let edited = match save_into(&text, &values, &lists) {
            Ok(edited) => edited,
            Err(why) => return self.refuse(format!("{} NOT saved — {why}", tilde(&path))),
        };
        // Checked as launch will read it, before anything is written.
        if let Err(e) = shannon_core::Config::parse(&edited) {
            let said = shannon_core::Config::describe_parse_error(&e, &edited);
            return self.refuse(format!(
                "{} NOT saved — it would not read ({said})",
                tilde(&path)
            ));
        }
        if let Err(e) = crate::private_file::replace(&path, &edited) {
            return self.refuse(format!("{} NOT saved — {e:#}", tilde(&path)));
        }
        info!(path = %path.display(), "config saved");
        let saved = format!("saved {}: {}", tilde(&path), said.join(", "));
        match (named.is_some(), trusted) {
            (true, _) => self.notify(saved),
            (false, true) => match self.config_stack.source(Origin::Project, &path, &edited) {
                Ok((config, notes)) => {
                    // Its rules come from the file now: listed once, under
                    // it — and the removals it says, too.
                    self.drop_session_rules_the_project_holds();
                    self.session_removed
                        .retain(|r| !matches!(r.origin, From::Shipped | From::Project));
                    self.adopt_config(config);
                    self.notify(saved);
                    for note in notes {
                        self.notify(note);
                    }
                }
                // Unreachable: parsed alone just above. Said if not.
                Err(_) => self.notify(format!("{saved} — not read back; `:reload` reads it")),
            },
            (false, false) => self.notify(format!("{saved} — not read here until you `:trust .`")),
        }
        if !left.is_empty() {
            self.notify(format!("not saved: {}", left.join("; ")));
        }
    }

    /// **The session's rules, as the project's file's lists** —
    /// `:config save`'s other half, folded into the `:set`s' `lists`: an
    /// `:allow`/`:ask`/`:deny` added to `rules`, a shipped rule `:un…`'d
    /// added to `remove`, one of the file's own `:un…`'d taken out, and
    /// what a project's file cannot say added to `left`. The file's own
    /// entries are read as rules (`Rule::same`): one it holds already is
    /// not added again, and one taken out is matched however its line was
    /// written — a `# comment` left it in the file while the message said
    /// "taken out" (review 2026-10-13, A).
    fn saved_rules(
        &self,
        mut lists: Vec<shannon_core::config::Listed>,
        mut left: Vec<String>,
    ) -> (Vec<shannon_core::config::Listed>, Vec<String>) {
        use shannon_core::permission::{Origin as From, Rule};
        let theirs = |key: &str| -> Vec<String> {
            self.config_stack
                .setters(key)
                .into_iter()
                .filter(|(origin, ..)| *origin == Origin::Project)
                .filter_map(|(_, _, value)| value.as_array())
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        };
        let lines_of = |lines: &[String], rule: &Rule| -> Vec<String> {
            lines
                .iter()
                .filter(|line| {
                    Rule::read_by(From::Project, line, &self.models)
                        .and_then(Result::ok)
                        .is_some_and(|r| r.same(rule))
                })
                .cloned()
                .collect()
        };
        let (file_rules, file_removes) =
            (theirs("permissions.rules"), theirs("permissions.remove"));
        let mut rules = shannon_core::config::Listed {
            path: "permissions.rules".into(),
            ..Default::default()
        };
        let mut remove = shannon_core::config::Listed {
            path: "permissions.remove".into(),
            ..Default::default()
        };
        for rule in &self.session_rules {
            let text = (rule.source.split_once(char::is_whitespace))
                .map_or(rule.source.as_str(), |(_, text)| text);
            if let Some(what) = self.credential_in_rule(text) {
                left.push(format!("a rule holding a credential ({what})"));
            } else if lines_of(&file_rules, rule).is_empty() {
                rules.add.push(rule.source.clone());
            }
        }
        for rule in &self.session_removed {
            match rule.origin {
                From::Shipped => {
                    if lines_of(&file_removes, rule).is_empty() {
                        remove.add.push(rule.source.clone());
                    }
                }
                From::Project => rules.take.extend(lines_of(&file_rules, rule)),
                From::Config | From::Session | From::Knowledge => left.push(format!(
                    "the removal of `{}` ({} — a project's file cannot remove it)",
                    rule.one_line(),
                    rule.origin.describe()
                )),
            }
        }
        for listed in [rules, remove] {
            if listed.add.is_empty() && listed.take.is_empty() {
                continue;
            }
            match lists.iter_mut().find(|l| l.path == listed.path) {
                Some(l) => {
                    for (into, from) in [(&mut l.add, listed.add), (&mut l.take, listed.take)] {
                        for entry in from {
                            if !into.contains(&entry) {
                                into.push(entry);
                            }
                        }
                    }
                }
                None => lists.push(listed),
            }
        }
        (lists, left)
    }

    /// **The session's rules a project's file now holds, dropped** — they
    /// are in force from the file, and `:rules` listed them twice, under the
    /// project and under this session (review 2026-10-13, A). After the file
    /// is read: `:config save`'s read-back, and a trust (`follow_trust`).
    pub(super) fn drop_session_rules_the_project_holds(&mut self) {
        use shannon_core::permission::{Origin as From, Rule};
        let held: Vec<Rule> = self
            .config_stack
            .project_rules()
            .iter()
            .filter_map(|line| Rule::read_by(From::Project, line, &self.models))
            .filter_map(Result::ok)
            .collect();
        self.session_rules
            .retain(|rule| !held.iter().any(|h| h.same(rule)));
    }

    /// **`:set`** — vim's option command over the config's dotted keys; the
    /// grammar and the typing are `shannon_core::config::set`'s, the layer is
    /// the stack's.
    ///
    /// Applies AT ONCE (Steven: "`:set tui.prompt.height=15` … should load
    /// right away. Same with any of our other options") through the same door
    /// as `:reload`.
    ///
    /// **By default it echoes what it set, where vim is silent**
    /// (`tui.set_report`: `default`; `quiet` is vim's silence, `verbose` the
    /// `:verbose` form). Most options change nothing you can see, and a
    /// command that changes nothing visible and says nothing reads as a dead
    /// one (`tools/drive/commands.py` hunts exactly that). The echo is the
    /// value you GOT — the `x?` line — so a value the config had to adjust
    /// shows as adjusted. Whatever the setting, an `x?` is answered (you
    /// asked), and a setting read only at launch says so: a `:set` cannot
    /// reach it, and it is gone when you quit.
    ///
    /// `verbose` is `:verbose set`: every line it says is in the verbose
    /// form.
    ///
    /// `Some` only for `:set all`, which is long enough to want the viewer.
    pub(super) fn set_command(&mut self, args: &str, verbose: bool) -> Option<CmdEffect> {
        let asked = if verbose {
            SetReport::Verbose
        } else {
            SetReport::Default
        };
        let args = args.trim();
        if args.is_empty() {
            let changed: Vec<String> = set::changed_options(&self.config)
                .iter()
                .map(|key| self.describe_option(key, asked))
                .collect();
            if changed.is_empty() {
                self.notify("every option is at its default — `:set all` lists them");
            } else {
                self.notify(changed.join("  ·  "));
            }
            return None;
        }
        if args == "all" {
            return Some(CmdEffect::View {
                label: ":set all",
                text: self.all_options(),
                kind: crate::editor::Filetype::Text,
            });
        }
        match set::parse(args) {
            Ok(items) => self.set_items(items, verbose),
            Err(why) => {
                self.refuse(why);
                None
            }
        }
    }

    /// **`:set`'s items, applied** — the one door a setting changes by, for
    /// the command line and for code that has the value already (`x` on the
    /// `:memories` screen: a name with a `,` or a space was split by
    /// re-parsing it, review 29). Plans, lays the layer on, says what it set.
    pub(super) fn set_items(&mut self, items: Vec<set::Item>, verbose: bool) -> Option<CmdEffect> {
        let asked = if verbose {
            SetReport::Verbose
        } else {
            SetReport::Default
        };
        let plan = match set::plan(items, &self.config) {
            Ok(plan) => plan,
            Err(why) => {
                self.refuse(why);
                return None;
            }
        };
        let set_keys = plan.set_keys();
        if !plan.table.is_empty() {
            match self.config_stack.set(plan.table) {
                Ok((config, notes)) => {
                    let changed = self.adopt_config(config);
                    // A `:set` of a kind's rule puts that kind's folds back
                    // to it even when the value is the one it had, as vim's
                    // `:set foldlevel` does: after `zR`,
                    // `:set tui.fold.thinking=shut` shuts the thoughts again,
                    // where it echoed and did nothing.
                    self.folds_to_their_rule(&crate::output::ruled_by_keys(&set_keys));
                    // **Not "takes effect at launch"**, which reads as "at the
                    // NEXT launch" — and a `:set` is forgotten by then
                    // (Steven, `:set tui.resume=last`). Say it does nothing
                    // now, is gone at exit, and where to put it instead.
                    let launch = read_at_launch(&changed);
                    if !launch.is_empty() {
                        let verb = if launch.len() == 1 { "is" } else { "are" };
                        self.notify(format!(
                            "{} {verb} read only at launch — this `:set` changes nothing now \
                             and is forgotten when you quit; to keep it, put it in your \
                             config (`:config edit`)",
                            launch.join(", ")
                        ));
                    }
                    for note in notes {
                        self.notify(note);
                    }
                    // The default, which a session's own effort outranks.
                    if set_keys.iter().any(|key| key == "turn.effort") {
                        self.say_whose_effort();
                    }
                }
                Err(why) => {
                    self.refuse(format!("{why} — nothing changed"));
                    return None;
                }
            }
        }
        // What was set, as the (new) `set_report` says; then what was asked,
        // always — each once.
        let report = if verbose {
            SetReport::Verbose
        } else {
            self.config.tui.set_report
        };
        let mut shown: Vec<String> = Vec::new();
        if report != SetReport::Quiet {
            shown.extend(set_keys.iter().map(|key| self.describe_option(key, report)));
        }
        for key in plan.show.iter().filter(|k| !set_keys.contains(k)) {
            shown.push(self.describe_option(key, asked));
        }
        if !shown.is_empty() {
            self.notify(shown.join("  ·  "));
        }
        None
    }

    /// `:verbose {cmd}` — vim's modifier. Only `:set` has more to say so far,
    /// and anything else is refused rather than run without it, so the prefix
    /// never silently means nothing.
    pub(super) fn verbose_command(&mut self, args: &str) -> Option<CmdEffect> {
        match crate::command::resolve(args) {
            crate::command::Resolved::Command {
                cmd: crate::command::Command::Set,
                args,
                ..
            } => self.set_command(args, true),
            _ => {
                self.refuse("`:verbose` works with `:set` so far — `:verbose set <option>?`");
                None
            }
        }
    }

    /// `key=value` — and, in the verbose form, every layer that sets it, top
    /// first, each with the value IT gives, down to the default:
    ///
    /// `tui.prompt.height=100 (:set 500, ~/.config/shannon/config.toml 20, default 10)`
    ///
    /// Vim's `:verbose` says only "last set from", which for an option you
    /// just typed is the one thing you already knew (Steven: "better to just
    /// state the default or what it is set in the config"). The layers under
    /// the top one are what `:unsource`, `:set x&` or quitting give back.
    fn describe_option(&self, key: &str, report: SetReport) -> String {
        let shown = set::shown(&self.config, key);
        if report != SetReport::Verbose {
            return shown;
        }
        let mut layers: Vec<String> = self
            .config_stack
            .setters(key)
            .into_iter()
            .map(|(origin, path, value)| {
                // Where a layer with no file came from: the environment's is
                // no `:set` — said as one, the confusion its origin was made
                // to end stood (review 2026-10-13, A, C).
                let from = match (path, origin) {
                    (Some(path), _) => tilde(path),
                    (None, Origin::Environment) => {
                        shannon_core::backend::claude_code::EFFORT_VAR.to_string()
                    }
                    (None, Origin::Tutor) => "the tutor".to_string(),
                    (None, _) => ":set".to_string(),
                };
                format!("{from} {}", set::spell(value))
            })
            .collect();
        layers.extend(set::default_of(key).map(|v| format!("default {}", set::spell(&v))));
        format!("{shown} ({})", layers.join(", "))
    }

    /// `:set all` as text: every option and its value.
    fn all_options(&self) -> String {
        let mut out = String::from(
            "--- Options ---\n\n\
             `:set <option>=<value>` changes one at once; `:verbose set <option>?`\n\
             says which file set it. Credentials and keymaps live in your config\n\
             file (`:config edit`), not here.\n\n",
        );
        for key in set::options(&self.config) {
            out.push_str(&set::shown(&self.config, &key));
            out.push('\n');
        }
        out
    }
}

/// What `:config edit` opens: the file, or [`SEED`] when there is none yet.
/// Its own function so the choice is testable without a `$EDITOR`.
///
/// **Only a missing file is seeded.** It used to seed on ANY read error, and
/// the seed is what gets saved: a config the reader refuses (too large, not
/// UTF-8, unreadable) would have opened as the seed and been overwritten by
/// it on `:wq` — your settings replaced by a comment block.
fn editable_config(path: &Path) -> Result<String, String> {
    Ok(shannon_core::config::read_file(path)?.unwrap_or_else(|| SEED.to_string()))
}

/// A path argument — `:source`/`:unsource`, `:trust`/`:untrust` — with a
/// leading `~` as your home, as vim reads one. Relative paths stay relative
/// (the directory Shannon runs in); the stack normalizes every spelling of a
/// file to one, and trust resolves what it keeps.
pub(super) fn config_path_arg(arg: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match (arg.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(arg),
    }
}

/// A path as you would type it — your home as `~` — the inverse of
/// [`config_path_arg`], so what `:verbose set` names is what `:unsource`
/// takes. A home of `/` is no home (every path would start with it). Setup
/// names the key's file the same way (`app::setup_flow`), and `:themes` its
/// headings (`theme_picker::place`).
pub(crate) fn tilde(path: &Path) -> String {
    tilde_in(path, std::env::var_os("HOME").map(PathBuf::from).as_deref())
}

/// [`tilde`] without the environment.
fn tilde_in(path: &Path, home: Option<&Path>) -> String {
    let home = home.filter(|h| h.parent().is_some());
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// **What a start at or above a home lets in, said at launch** (review stop
/// 12, Q8; Steven 10-27: "a launch note is a good idea… include `~` in that
/// too"). Inside where Shannon started, a read or a write no rule names
/// needs no question (`permission::decide`'s `path_default`; a delete and an
/// exec ask). Started at `~`, that is all of your home; started above it
/// (`/`, `/Users`, `/home`), every home under it — and the credentials the
/// gate guards by path are YOURS (`~/.ssh/config`…), so another user's are
/// guarded only by name (`id_rsa`, `*.pem`, `.env*`). Nothing is changed:
/// the start is the user's choice, said.
pub(crate) fn start_note(start: &Path, home: &Path) -> Option<String> {
    if start == home {
        Some(
            "started in your home directory: all of ~ is inside this project, so a read or a \
             write anywhere in it needs no question (a delete, and the guarded credential \
             paths, still ask) — start in a project's directory to keep the rest of ~ out"
                .to_string(),
        )
    } else if home.starts_with(start) {
        Some(format!(
            "started at {}, above your home: every home under it is inside this project, so a \
             read or a write there needs no question — and another user's credentials \
             (their ~/.ssh/config, ~/.aws/config…) are guarded only by name, not by path",
            start.display()
        ))
    } else {
        None
    }
}

/// The launch-describing settings, as `:help config` names them: the last
/// part of each key, backticked, joined with "and".
#[cfg(test)]
pub(super) fn launch_list() -> String {
    LAUNCH_KEYS
        .iter()
        .map(|key| format!("`{}`", key.rsplit('.').next().unwrap_or(key)))
        .collect::<Vec<_>>()
        .join(" and ")
}

/// Every relaunch-only setting, as `:help config` lists them.
#[cfg(test)]
pub(super) fn relaunch_list() -> String {
    let mut words: Vec<&str> = Vec::new();
    for (_, word) in RELAUNCH_KEYS {
        if !words.contains(word) {
            words.push(word);
        }
    }
    words.join(", ")
}

/// Of the changed keys, the relaunch-only settings they touch, in
/// `RELAUNCH_KEYS` order and without repeats.
fn relaunch_words(changed: &[String]) -> Vec<&'static str> {
    let mut words: Vec<&'static str> = Vec::new();
    for (prefix, word) in RELAUNCH_KEYS {
        let touched = changed.iter().any(|k| key_is_under(k, prefix));
        if touched && !words.contains(word) {
            words.push(word);
        }
    }
    words
}

/// **The line naming what of `changed` only a relaunch applies**, if any
/// — a file's report's second half (`report_change`, and the trust
/// sentence's, `trust_flow`).
pub(super) fn relaunch_note(changed: &[String]) -> Option<String> {
    let later = relaunch_words(changed);
    (!later.is_empty()).then(|| format!("these apply on relaunch: {}", later.join(", ")))
}

/// `n` of `noun`, plural past one: "1 rule", "2 rules".
fn count(n: usize, noun: &str) -> String {
    match n {
        1 => format!("1 {noun}"),
        n => format!("{n} {noun}s"),
    }
}

/// **What one list's change says** in `:config save`'s report: "2 rules",
/// "1 shipped rule removed", "skills.paths (1 added, 1 taken out)".
fn said_of(listed: &shannon_core::config::Listed) -> String {
    let (add, take) = (listed.add.len(), listed.take.len());
    let parts: Vec<String> = match listed.path.as_str() {
        "permissions.rules" => [
            (add > 0).then(|| count(add, "rule")),
            (take > 0).then(|| format!("{take} of its rules taken out")),
        ]
        .into_iter()
        .flatten()
        .collect(),
        "permissions.remove" => [
            (add > 0).then(|| format!("{} removed", count(add, "shipped rule"))),
            (take > 0).then(|| format!("{} put back", count(take, "shipped rule"))),
        ]
        .into_iter()
        .flatten()
        .collect(),
        path => {
            let what: Vec<String> = [
                (add > 0).then(|| format!("{add} added")),
                (take > 0).then(|| format!("{take} taken out")),
            ]
            .into_iter()
            .flatten()
            .collect();
            vec![format!("{path} ({})", what.join(", "))]
        }
    };
    parts.join(", ")
}

/// **`text` — a project's `.shannon/config.toml`, or nothing yet — with
/// this session's settings written in** (`config_save`): each of `values`
/// at its key, and each list changed by what was added and taken out
/// ([`put_list`]) — never written whole. Edited in place, as setup writes
/// `[tui] model` (`setup_flow::persist_provider`), so comments, order and
/// every other line are kept.
fn save_into(
    text: &str,
    values: &toml::Table,
    lists: &[shannon_core::config::Listed],
) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|_| "it does not parse as it stands — fix it first".to_string())?;
    put_options(doc.as_table_mut(), values, false)?;
    for listed in lists {
        put_list(doc.as_table_mut(), listed)?;
    }
    Ok(doc.to_string())
}

/// **One list changed in place**: each of `take` taken out, each of `add`
/// added unless there already — the rest, another file's or the team's,
/// kept. Written whole, `:set skills.paths+=x` wiped the team's entries
/// from the project's list (review 2026-10-13, A).
fn put_list(
    root: &mut dyn toml_edit::TableLike,
    listed: &shannon_core::config::Listed,
) -> Result<(), String> {
    let (sections, key) = listed
        .path
        .rsplit_once('.')
        .unwrap_or(("", listed.path.as_str()));
    let mut at = root;
    let mut inline = false;
    for name in sections.split('.').filter(|s| !s.is_empty()) {
        (at, inline) = section(at, name, inline)?;
    }
    let list = at
        .entry(key)
        .or_insert(toml_edit::value(toml_edit::Array::new()))
        .as_array_mut()
        .ok_or_else(|| format!("`{}` is not a list", listed.path))?;
    list.retain(|v| {
        v.as_str()
            .is_none_or(|s| !listed.take.iter().any(|t| t == s))
    });
    for entry in &listed.add {
        if !list.iter().any(|v| v.as_str() == Some(entry)) {
            list.push(entry.as_str());
        }
    }
    Ok(())
}

/// **The table at `key` in `at`, made if there is none**, and whether it is
/// inline. Inside an inline table (`tui = { scrolloff = 2 }`) a table made
/// is inline too: a `[table]` there is never written out, and the keys put
/// in it were lost while the save said they were saved (review 2026-10-13,
/// A).
fn section<'t>(
    at: &'t mut dyn toml_edit::TableLike,
    key: &str,
    inline: bool,
) -> Result<(&'t mut dyn toml_edit::TableLike, bool), String> {
    let made = match inline {
        true => {
            toml_edit::Item::Value(toml_edit::Value::InlineTable(toml_edit::InlineTable::new()))
        }
        false => toml_edit::table(),
    };
    let item = at.entry(key).or_insert(made);
    let inner_inline = item.is_inline_table();
    let table = item
        .as_table_like_mut()
        .ok_or_else(|| format!("`{key}` is not a table"))?;
    Ok((table, inner_inline))
}

/// `values` written into `at`, key by key: a table walked, any other value
/// set — in TOML's own spelling, read back as `toml_edit`'s. `inline`: `at`
/// is an inline table.
fn put_options(
    at: &mut dyn toml_edit::TableLike,
    values: &toml::Table,
    inline: bool,
) -> Result<(), String> {
    for (key, value) in values {
        match value {
            toml::Value::Table(inner) => {
                let (table, inline) = section(at, key, inline)?;
                put_options(table, inner, inline)?
            }
            value => {
                let value: toml_edit::Value = value
                    .to_string()
                    .parse()
                    .map_err(|_| format!("`{key}` could not be written"))?;
                at.insert(key, toml_edit::Item::Value(value));
            }
        }
    }
    Ok(())
}

/// Of the changed keys, the ones read only at launch — relaunch-only and
/// launch-describing alike, which to a `:set` are the same thing: out of its
/// reach now, and gone when you quit.
fn read_at_launch(changed: &[String]) -> Vec<&str> {
    let only_at_launch = |key: &str| {
        RELAUNCH_KEYS.iter().any(|(p, _)| key_is_under(key, p))
            || LAUNCH_KEYS.iter().any(|p| key_is_under(key, p))
    };
    changed
        .iter()
        .map(String::as_str)
        .filter(|key| only_at_launch(key))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A start at `~`, or above it, is said at launch; any other is not**
    /// (review stop 12, Q8). Above: `/` and the directory holding the homes;
    /// inside a home, or beside it, nothing.
    #[test]
    fn a_start_that_puts_homes_inside_is_said() {
        let home = Path::new("/Users/someone");
        let at_home = start_note(home, home).expect("~ itself");
        assert!(at_home.contains("all of ~ is inside"), "{at_home}");
        for above in ["/", "/Users"] {
            let said = start_note(Path::new(above), home).expect(above);
            assert!(
                said.starts_with(&format!("started at {above}, above your home")),
                "{said}"
            );
            assert!(said.contains("another user's credentials"), "{said}");
        }
        for elsewhere in [
            "/Users/someone/project",
            "/Users/other",
            "/tmp",
            "/Users/some",
        ] {
            assert_eq!(start_note(Path::new(elsewhere), home), None, "{elsewhere}");
        }
    }

    /// **`:config save` changes a list in place, never writes it whole**
    /// (review 2026-10-13, A): the team's entries stay, what was taken out
    /// goes, nothing is added twice; and a key under an inline table is
    /// written inline, where a `[table]` was dropped while the save said
    /// it was saved.
    #[test]
    fn a_save_edits_the_file_in_place() {
        use shannon_core::config::Listed;
        let text = "# the team's\ntui = { scrolloff = 2 }\n\
                    [skills]\npaths = [\"/srv/team\", \"/srv/old\"]\n\
                    [permissions]\nrules = [\"allow x\"]\n";
        let lists = [
            Listed {
                path: "skills.paths".into(),
                add: vec!["/srv/mine".into()],
                take: vec!["/srv/old".into()],
            },
            Listed {
                path: "permissions.rules".into(),
                add: vec!["allow x".into(), "allow y".into()],
                take: Vec::new(),
            },
        ];
        let values: toml::Table = toml::from_str("[tui.prompt]\nheight = 5\n").unwrap();
        let saved = save_into(text, &values, &lists).unwrap();
        assert!(saved.starts_with("# the team's"), "comments kept:\n{saved}");
        let read = Config::parse(&saved).unwrap();
        assert_eq!(read.skills.paths, ["/srv/team", "/srv/mine"], "{saved}");
        assert_eq!(read.permissions.rules, ["allow x", "allow y"], "{saved}");
        assert_eq!(read.tui.scrolloff, 2, "{saved}");
        assert_eq!(read.tui.prompt.height, 5, "kept inline:\n{saved}");
    }

    /// **A new config starts with no settings in it** — a pointer, not a copy
    /// of the defaults, since every copied default would override the better
    /// one a later release ships (`:h defaults.vim`). An existing file opens
    /// as itself.
    #[test]
    fn a_new_config_starts_empty_of_settings() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = editable_config(&dir.path().join("absent.toml")).unwrap();
        assert_eq!(fresh, SEED);
        let active: Vec<&str> = fresh
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
            .collect();
        assert!(active.is_empty(), "the seed sets something: {active:?}");
        assert_eq!(
            Config::parse(&fresh).expect("the seed parses"),
            Config::default(),
            "so it IS the defaults, by being empty"
        );

        let mine = dir.path().join("mine.toml");
        std::fs::write(&mine, "[tui]\nmouse = false\n").unwrap();
        assert_eq!(editable_config(&mine).unwrap(), "[tui]\nmouse = false\n");

        // A file that is there but refused is NOT the seed: the seed is what
        // gets saved, over your settings.
        let huge = dir.path().join("huge.toml");
        std::fs::write(
            &huge,
            "#".repeat(shannon_core::config::MAX_CONFIG_BYTES as usize + 1),
        )
        .unwrap();
        assert!(editable_config(&huge).is_err());
        assert!(editable_config(dir.path()).is_err(), "a directory");
    }

    /// A path shows your home as `~` — at a component boundary only, and
    /// never for a home of `/`.
    #[test]
    fn a_path_shows_home_as_tilde() {
        let home = Some(Path::new("/Users/me"));
        let t = |p: &str| tilde_in(Path::new(p), home);
        assert_eq!(
            t("/Users/me/.config/shannon/config.toml"),
            "~/.config/shannon/config.toml"
        );
        assert_eq!(t("/Users/me"), "~");
        assert_eq!(
            t("/Users/meg/x.toml"),
            "/Users/meg/x.toml",
            "not a prefix of text"
        );
        assert_eq!(t("/etc/x.toml"), "/etc/x.toml");
        assert_eq!(
            tilde_in(Path::new("/etc/x"), Some(Path::new("/"))),
            "/etc/x"
        );
        assert_eq!(tilde_in(Path::new("/etc/x"), None), "/etc/x");
    }

    #[test]
    fn a_change_names_only_the_relaunch_settings_it_touched() {
        let changed = |keys: &[&str]| keys.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        assert_eq!(
            relaunch_words(&changed(&["tui.mouse", "tui.scrolloff"])),
            ["mouse"]
        );
        assert_eq!(
            relaunch_words(&changed(&["keymap.normal.Q", "secrets.alias_home"])),
            ["keymaps", "aliases"]
        );
        // Every entry answers for itself: two share the word "aliases", so the
        // manual's list cannot tell if one is dropped (Reviewer C).
        assert_eq!(relaunch_words(&changed(&["alias"])), ["aliases"]);
        assert_eq!(
            relaunch_words(&changed(&["secrets.env"])),
            ["[secrets] env"]
        );
        assert!(
            relaunch_words(&changed(&["tui.mouse_wheel_speed"])).is_empty(),
            "a key that only STARTS with a relaunch key's text is not it"
        );
    }
}
