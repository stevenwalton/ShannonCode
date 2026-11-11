//! **Trust, wired** — where `crate::trust`'s answer stops a `claude` process
//! from starting, where the question is asked, `:trust`/`:untrust`, and the
//! project's own `.shannon/` files following the answer (`follow_trust`).
//!
//! Two halves, because the question can only be asked where someone is
//! asking for something: a prompt about to be sent (`submit_prompt`). Every
//! other way a Claude Code request starts — a `:continue`, a retry, a job,
//! a distillation, a queued prompt — meets the refusal at the one door every
//! request leaves through (`spawn_tagged`), which says how to trust.
//!
//! A child module of `app`, as `setup_flow` is.

use std::path::Path;

use super::config_file::{config_path_arg, tilde};
use super::*;
use crate::setup::TrustAnswer;
use crate::trust::{Lasting, Trusted};

impl App {
    /// **The kept trust** — launch's (`main`), before anything can be sent.
    pub fn set_trusted(&mut self, trusted: Trusted) {
        self.trusted = trusted;
    }

    /// **The Claude Code sessions Shannon started, kept** — launch's
    /// (`main`): a restart resumes them rather than re-sending every history.
    pub fn set_claude_sessions(&mut self, sessions: crate::claude_sessions::ClaudeSessions) {
        self.claude_sessions = sessions;
    }

    /// **Why a request for `model` may not start here**, if it may not: it
    /// would run Claude Code in a directory not trusted. Asked by the door
    /// every request leaves through (`spawn_tagged`).
    pub(super) fn claude_code_refusal(&self, model: Option<&str>) -> Option<String> {
        if route(model) != Route::ClaudeCode {
            return None;
        }
        // Where Claude Code runs: where Shannon started (`App::start`), the
        // one answer the gate's hook check reads too.
        match self.start.as_deref() {
            Some(dir) if self.trusted.covers(dir) => None,
            Some(dir) => Some(format!(
                "Claude Code may not run in {} yet — it would run this project's own Claude \
                 Code settings (hooks, MCP servers); `:trust` trusts it",
                tilde(dir)
            )),
            None => Some(
                "Shannon cannot tell which directory it is in, so Claude Code may not run".into(),
            ),
        }
    }

    /// **Ask whether Claude Code may run here before anything is typed** —
    /// at launch (`main`), when setup sets Claude Code up, and when `:model`
    /// switches to it. Claude Code and Copilot ask before their session
    /// starts; asked at the first send instead, the question arrived after
    /// the message was written (Steven, 10-04). Only when the session the
    /// prompt goes to answers from Claude Code (the session's model, so a
    /// choice of Demo or the API asks nothing). An `n` leaves it to the
    /// send-time ask.
    ///
    /// **Where no screen asks, what the directory holds unread is said**
    /// instead — the screen is the question, and a note under it was stale
    /// the moment it was answered yes. One door for the three: a launch that
    /// opened setup and chose the API never said it (review 2026-10-13, A).
    pub fn ask_trust_up_front(&mut self) {
        if !self.ask_trust_for(&[self.prompt_target()], false) {
            self.say_unread_project();
        }
    }

    /// **Say what this directory holds that is not read** — what its
    /// `.shannon/` holds while it is not trusted (`Trusted::unread_project`)
    /// — so a project's file is never silently not applied (Steven, 10-13).
    /// Where no trust screen asks (`ask_trust_up_front`, setup left); the
    /// screen names it, and its no leads with it (`answer_trust`).
    pub(super) fn say_unread_project(&mut self) {
        let note = self
            .start
            .as_deref()
            .and_then(|start| self.trusted.unread_project(start));
        if let Some(note) = note {
            self.notify(note);
        }
    }

    /// **Ask before a prompt starts Claude Code in an untrusted directory** —
    /// `true` when the question was put, and the prompt must wait for it.
    /// Every session the prompt goes to counts: the marked ones, or the one
    /// it is aimed at.
    pub(super) fn ask_trust_before_sending(&mut self) -> bool {
        let targets: Vec<BufferId> = match self.workspace.marked_panes() {
            marked if marked.is_empty() => vec![self.prompt_target()],
            marked => (marked.iter())
                .filter_map(|&p| self.session_for_pane(p).map(|b| b.id))
                .collect(),
        };
        self.ask_trust_for(&targets, true)
    }

    /// **The one question**, for whichever asks: the trust screen, put when
    /// any of `targets` answers from Claude Code and this directory is not
    /// trusted. `held`: a prompt waits on the answer. `true` when it was put.
    ///
    /// A screen, not a y/n on the status line (Steven, 10-04: "for the
    /// directory trust I'd like a screen. And the 3 options"). Modal like any
    /// setup screen, so it is answered before anything is typed.
    fn ask_trust_for(&mut self, targets: &[BufferId], held: bool) -> bool {
        let runs_claude_code = targets
            .iter()
            .any(|&b| route(Some(&self.session_model(b))) == Route::ClaudeCode);
        // A setup screen already up is answered first; the door still refuses.
        if !runs_claude_code || self.setup.is_some() {
            return false;
        }
        match self.start.clone() {
            Some(dir) if self.trusted.covers(&dir) => false,
            Some(dir) => {
                let shown = tilde(&dir);
                let holds = crate::trust::project_held(&dir);
                self.setup = Some(crate::setup::Setup::trust(dir, shown, held, holds));
                self.chat_dirty = true;
                true
            }
            // No directory to name: the door refuses, and says why.
            None => false,
        }
    }

    /// **The trust screen answered** — trust `dir` as `answer` says, or say
    /// what not trusting means; then send the prompt that waited, if one did
    /// and the answer let it.
    pub(super) fn answer_trust(&mut self, dir: &Path, answer: TrustAnswer, held: bool) {
        let trusted = match answer {
            TrustAnswer::Remember => self.trust_dir(dir, Lasting::Remembered),
            TrustAnswer::ThisRun => self.trust_dir(dir, Lasting::ThisRun),
            TrustAnswer::No => {
                // What stays unread leads: said after the Claude Code half,
                // it ran past the row's cut (review 2026-10-13, C).
                let unread = (self.start.as_deref())
                    .and_then(|start| self.trusted.unread_names(start))
                    .map(|(names, is)| format!("{names} {is} not read, and "))
                    .unwrap_or_default();
                let said = if held {
                    format!(
                        "not sent — {unread}Claude Code may not run in this directory yet; \
                         your message is still in the prompt (`:trust` trusts it)"
                    )
                } else {
                    format!(
                        "not trusted — {unread}Claude Code may not run in this directory yet; \
                         sending asks again (`:trust` trusts it, `:model` picks another backend)"
                    )
                };
                self.post_first(NoticeKind::Report, said);
                false
            }
        };
        if trusted && held {
            self.submit_prompt();
        }
    }

    /// **Trust `dir`** for as long as `lasting` says, and say so — the
    /// screen's yes and `:trust` both — and read the project's own files
    /// when that trusts where Shannon started. `false` (and said) when it
    /// could not be kept.
    ///
    /// **One sentence, led by what was read**: said as two notices, a long
    /// "trusted …" first, the row's cut hid "read … — changed: …" behind it
    /// (Steven, 10-13); in one sentence that began "trusted <dir> until
    /// Shannon quits — ", it still did at 80 columns (review 2026-10-13, C).
    /// A project's file that would not read makes the sentence a refusal
    /// naming why — it said "NOT LOADED", then "its own settings apply
    /// there" (review 2026-10-13, A).
    pub(super) fn trust_dir(&mut self, dir: &Path, lasting: Lasting) -> bool {
        match self.trusted.trust(dir, lasting) {
            Ok(dir) => {
                let how_long = match lasting {
                    Lasting::Remembered => "",
                    Lasting::ThisRun => " until Shannon quits",
                };
                let followed = self.follow_trust(true);
                let trusted = format!("trusted {}{how_long} (`:untrust` revokes)", tilde(&dir));
                let said = match followed.did {
                    Some(did) => format!("{did} — {trusted}"),
                    None => format!("{trusted} — its own settings apply there"),
                };
                // First on the row: the answer, before the launch notes the
                // screen covered (Steven, 10-14).
                let kind = match followed.failed {
                    true => NoticeKind::Refusal,
                    false => NoticeKind::Report,
                };
                self.post_first(kind, said);
                for line in followed.then {
                    self.notify(line);
                }
                true
            }
            Err(why) => {
                self.refuse(why);
                false
            }
        }
    }

    /// **`:trust [dir]`** — with a directory (`.` for here), trust it; with
    /// none, say what is trusted and whether here is.
    pub(crate) fn trust_command(&mut self, arg: &str) {
        let arg = arg.trim();
        if !arg.is_empty() {
            self.trust_dir(&config_path_arg(arg), Lasting::Remembered);
            return;
        }
        let here_trusted = self
            .start
            .as_deref()
            .is_some_and(|d| self.trusted.covers(d));
        let listed: Vec<String> = self
            .trusted
            .dirs()
            .iter()
            .map(|d| tilde(d))
            .chain(
                self.trusted
                    .this_run()
                    .iter()
                    .map(|d| format!("{} (this run)", tilde(d))),
            )
            .collect();
        self.notify(match (listed.is_empty(), here_trusted) {
            (true, _) => "no directory is trusted — `:trust .` trusts this one".to_string(),
            (false, true) => format!("trusted: {} — this directory is", listed.join(", ")),
            (false, false) => format!(
                "trusted: {} — this directory is not (`:trust .` trusts it)",
                listed.join(", ")
            ),
        });
    }

    /// **`:untrust [dir]`** — stop trusting what covers `dir` (here, with
    /// none), and say what was removed.
    pub(crate) fn untrust_command(&mut self, arg: &str) {
        let arg = arg.trim();
        let dir = match arg {
            "" => match self.start.clone() {
                Some(dir) => dir,
                None => return self.refuse("which directory is this? Shannon cannot tell"),
            },
            arg => config_path_arg(arg),
        };
        match self.trusted.revoke(&dir) {
            Ok(gone) if gone.is_empty() => {
                self.notify(format!("{} was not trusted — nothing changed", tilde(&dir)))
            }
            Ok(gone) => {
                let names: Vec<String> = gone.iter().map(|d| tilde(d)).collect();
                let followed = self.follow_trust(false);
                let names = names.join(", ");
                self.notify(match followed.did {
                    Some(did) => {
                        format!("{did} — no longer trusted: {names}; Claude Code asks again there")
                    }
                    None => format!(
                        "no longer trusted: {names} — its own settings no longer apply, and \
                         Claude Code asks again there"
                    ),
                });
                for line in followed.then {
                    self.notify(line);
                }
            }
            Err(why) => self.refuse(why),
        }
    }

    /// **The project's own files follow its trust** (6d) — after the
    /// screen's yes, `:trust` and `:untrust`, as though Shannon had started
    /// so: a trusted start's `.shannon/config.toml` goes on the stack where
    /// launch puts it (right above yours), an untrusted one's comes back
    /// off, and its `.shannon/models/` are read again or dropped (by
    /// `adopt_config` → `load_models`) — with or without a config file.
    ///
    /// Only the trust at launch counted: a yes on the trust screen left the
    /// project's file unread until the next launch, and `:untrust` left its
    /// rules deciding (Steven, 10-13).
    ///
    /// `trusting`: whether this follows a trust or a revoke — what the
    /// models' change is called. Returns what moved, for the caller's
    /// sentence ([`Followed`]).
    fn follow_trust(&mut self, trusting: bool) -> Followed {
        let Some(start) = self.start.clone() else {
            return Followed::default();
        };
        let loaded = self.config_stack.project_file().map(Path::to_path_buf);
        let mut failed = None;
        let moved = match (self.trusted.project_file(&start), loaded) {
            (Some(path), None) => match self.read_project_file(&path) {
                Ok(done) => done.map(|done| ("read .shannon/config.toml", done)),
                Err(why) => {
                    failed = Some(format!(".shannon/config.toml NOT LOADED ({why})"));
                    None
                }
            },
            (None, Some(path)) => self
                .config_stack
                .unload(&path)
                .map(|done| ("took .shannon/config.toml off", done)),
            _ => None,
        };
        let Some((did, (config, notes))) = moved else {
            // No file moved: the models still follow the trust.
            let config = (*self.config).clone();
            let changed = self.adopt_config(config);
            let models = (!changed.is_empty()).then(|| match trusting {
                true => format!("read {}", changed.join(", ")),
                false => format!("took {} off", changed.join(", ")),
            });
            let did: Vec<String> = failed.iter().cloned().chain(models).collect();
            return Followed {
                did: (!did.is_empty()).then(|| did.join("; ")),
                then: Vec::new(),
                failed: failed.is_some(),
            };
        };
        // A session rule the file now holds comes from it: listed once (a
        // `:config! save` before this trust listed them twice).
        self.drop_session_rules_the_project_holds();
        let changed = self.adopt_config(config);
        let did = match changed.is_empty() {
            true => format!("{did}, which changed nothing"),
            false => format!("{did} (changed: {})", changed.join(", ")),
        };
        let then = super::config_file::relaunch_note(&changed)
            .into_iter()
            .chain(notes)
            .collect();
        Followed {
            did: Some(did),
            then,
            failed: false,
        }
    }

    /// **Keep what the stack last said a project's file changes about
    /// safety** (`ConfigStack::told`), so the next launch says it only when
    /// it is news (Steven, 10-19). From the doors every read of the project's
    /// file ends in: launch's `set_config_stack`, and `adopt_config` after
    /// the trust screen, `:trust`, `:reload` and `:config edit`.
    pub(super) fn keep_told(&mut self) {
        let Some((file, digest)) = self.config_stack.told() else {
            return;
        };
        if self.trusted.told(file) == Some(digest) {
            return;
        }
        let (file, digest) = (file.to_path_buf(), digest.to_string());
        // What failed leads, and that no trust went with it: the save's own
        // words ("the trusted directories were not saved") read as a lost
        // trust, where the trusts kept are as they were (review stop 6, F 9).
        if let Err(why) = self.trusted.tell(&file, &digest) {
            self.notify(format!(
                "the project's safety note could not be marked as said, so it is said again \
                 next launch; your trusted directories are as they were ({why})"
            ));
        }
    }

    /// **The project's file, put on the stack** — the stack's config and
    /// notes; `None` for no file there (a normal state); or why it could
    /// not be read or parsed, for the trust's sentence to say.
    fn read_project_file(
        &mut self,
        path: &Path,
    ) -> Result<Option<(shannon_core::Config, Vec<String>)>, String> {
        let Some(text) =
            shannon_core::config::read_file(path).map_err(|why| format!("not read: {why}"))?
        else {
            return Ok(None);
        };
        let project = shannon_core::config::Origin::Project;
        self.config_stack
            .source(project, path, &text)
            .map(Some)
            // The line number, never the line — see `save_config`.
            .map_err(|e| shannon_core::Config::describe_parse_error(&e.0, &text))
    }
}

/// **What a trust moved**, for the sentence that says it (`trust_dir`,
/// `untrust_command`).
#[derive(Default)]
struct Followed {
    /// What moved, the sentence's lead: "read .shannon/config.toml
    /// (changed: …)".
    did: Option<String>,
    /// Said after: what only a relaunch applies, the file's own notes.
    then: Vec<String>,
    /// The project's file would not read: the sentence is a refusal.
    failed: bool,
}
