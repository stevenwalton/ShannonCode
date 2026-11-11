//! **The running configuration, built the way vim builds it**: compiled
//! defaults, then every file applied in order, each a sparse diff over what
//! came before (`:h initialization`, `:h :source`).
//!
//! A file is a *layer*. Launch applies the system file (the first
//! `shannon/config.toml` in `$XDG_CONFIG_DIRS`, Neovim's `sysinit.vim` rule)
//! and then the user's; `:source {file}` and `:reload` put another on top.
//! The running config is a pure function of the layers: fold them over the
//! defaults, later keys winning, and deserialize.
//!
//! **Faithful to vim's `:source`, which is additive.** Sourcing runs a file's
//! settings against the CURRENT state; it does not rebuild from defaults. A
//! key deleted from a file and the file sourced again keeps its old value,
//! because what the file said before is still in the stack — re-sourcing lays
//! the new text over the old (into the same layer, when nothing came between),
//! it does not replace it. `:reload` is `:source` of your own config, so the
//! same holds.
//!
//! **And unloadable, which vim is not** (Steven: project files you can take
//! off again). Because nothing is applied destructively, unloading a file is
//! taking its layers out of the stack and folding again: its keys fall back to
//! whatever the remaining layers say, in their order, down to the defaults.
//!
//! What merges: tables merge key by key, at every depth (`[keymap.normal]`
//! from a project file adds to yours rather than replacing it); anything
//! else — a string, a number, an array, `[[alias]]` — is replaced whole by
//! the later layer, as a `:set` replaces an option's value. The exception is
//! a list of independent statements ([`STACKED`]: `[permissions] rules` and
//! `remove`, the `[memory]`/`[skills]` locations, what is disabled), which
//! every FILE adds to — and a `:set` replaces, as it was built from the
//! whole list in force (`x-=v` takes `v` out wherever it came from).
//!
//! **What is loaded is on the record.** Every layer put on or taken off is
//! logged with the keys it changed, and a file that is not yours
//! ([`Origin::is_yours`]) that changes a safety setting
//! ([`SAFETY_KEYS`](super::SAFETY_KEYS)) says so among its notes.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use super::{Config, SAFETY_KEYS, key_is_under};

/// The directory a project keeps Shannon's files in, under where Shannon
/// started: its config ([`project_path`]) and its program models.
pub const PROJECT_DIR: &str = ".shannon";

/// Where a project keeps its config: `<start>/.shannon/config.toml`. Read
/// only where the start is trusted — the caller's to know, as the trust
/// store is the app's.
pub fn project_path(start: &Path) -> PathBuf {
    start.join(PROJECT_DIR).join("config.toml")
}

/// Where a project keeps its program models: `<start>/.shannon/models/`,
/// read on the same condition as its config.
pub fn project_models_dir(start: &Path) -> PathBuf {
    start.join(PROJECT_DIR).join("models")
}

/// Where a layer came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The machine-wide file, read first at launch (`$XDG_CONFIG_DIRS`).
    System,
    /// Your config (`$XDG_CONFIG_HOME/shannon/config.toml`) — at launch, and
    /// again on every `:reload`.
    User,
    /// The project's (`<start>/.shannon/config.toml`), read after yours —
    /// its settings win where both speak (Steven, 10-13: system, global,
    /// project) — and only where Shannon started in a trusted directory.
    /// It stands right above the latest read of your files, wherever it
    /// arrives from (launch, the trust screen, `:trust`, `:reload`), so a
    /// `:set` typed before it still wins over it, as at launch.
    /// Not yours: what only your files set stays yours (`YOURS_ONLY`),
    /// and a safety setting it changes is said at load.
    Project,
    /// Anything else, by `:source {file}`.
    Sourced,
    /// Typed: `:set` (vim's "Last set from command line"). With
    /// [`Environment`](Self::Environment), an origin with no file, so one
    /// `:scriptnames` does not list and `:unsource` cannot name.
    CommandLine,
    /// Launch's environment: `CLAUDE_CODE_EFFORT_LEVEL`, laid as a `:set`
    /// typed before anything else ([`ConfigStack::start_effort`]) — but not
    /// typed, so `:config save` does not write it into a project's file,
    /// and `:verbose set` says where it came from.
    Environment,
    /// **`shannon tutor`'s own settings** — the stub, a short prompt — laid
    /// at launch over no file at all ([`ConfigStack::start_tutor`]): the
    /// tutor reads none of yours, as vimtutor runs vim with `-u NONE`, so
    /// no `:reload` can lay a model of yours back over its stub. Not typed,
    /// so `:config save` writes none of it.
    Tutor,
}

impl Origin {
    /// The word the listing shows.
    pub fn word(self) -> &'static str {
        match self {
            Origin::System => "system",
            Origin::User => "user",
            Origin::Project => "project",
            Origin::Sourced => "sourced",
            Origin::CommandLine => "command line",
            Origin::Environment => "environment",
            Origin::Tutor => "tutor",
        }
    }

    /// **Is this a file you set Shannon up with** — yours, or your machine's
    /// (the system file an administrator installs)? A file you `:source` is
    /// not: it is often someone else's writing, which is when "it changed
    /// redaction" should not be one key in a list.
    ///
    /// Decides whether a layer that changes a safety setting
    /// ([`SAFETY_KEYS`]) is called out. A `match` rather than `matches!`, so a
    /// new origin does not compile until someone decides which it is.
    pub fn is_yours(self) -> bool {
        match self {
            // You typed it, or set it where you launched.
            Origin::System
            | Origin::User
            | Origin::CommandLine
            | Origin::Environment
            | Origin::Tutor => true,
            // A repository's own: its rules are yours to keep (Steven,
            // 10-13), its write places and `PATH` are not.
            Origin::Project | Origin::Sourced => false,
        }
    }
}

/// One file's settings, as parsed — only the keys it wrote — or one run of
/// `:set`s.
#[derive(Debug, Clone)]
struct Layer {
    origin: Origin,
    /// The file, in its one spelling ([`file_key`]). `None` exactly for
    /// [`Origin::CommandLine`], [`Origin::Environment`] and
    /// [`Origin::Tutor`]: [`ConfigStack::source`] always gives one, and
    /// [`ConfigStack::set`], [`ConfigStack::start_effort`] and
    /// [`ConfigStack::start_tutor`] never do — the only four builders.
    path: Option<PathBuf>,
    table: toml::Table,
}

impl Layer {
    /// What the log and the notes call it.
    fn name(&self) -> String {
        match &self.path {
            Some(path) => path.display().to_string(),
            None => format!("the {}", self.origin.word()),
        }
    }
}

/// **This session's `:set`s as a project's file would say them**
/// ([`ConfigStack::typed_for_project`]) — what `:config save` writes.
#[derive(Debug, Default, PartialEq)]
pub struct Typed {
    /// Each key holding one value that a `:set` named, at its value in force.
    pub values: toml::Table,
    /// Each list the files add to that a `:set` named: what to add to the
    /// project's, and what to take out of its own entries.
    pub lists: Vec<Listed>,
    /// What a project's file cannot say, named for the user.
    pub left: Vec<String>,
}

/// One list's change, by its dotted key (`skills.paths`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Listed {
    pub path: String,
    pub add: Vec<String>,
    pub take: Vec<String>,
}

/// The layers, bottom first, and the config they add up to.
#[derive(Debug, Clone, Default)]
pub struct ConfigStack {
    layers: Vec<Layer>,
    /// **What the project's file was last said to change about safety** —
    /// its file and its settings' digests ([`safety_digests`], as one token)
    /// — so its note is said when you trust it and when what it says changes,
    /// naming what did, not at every launch (Steven, 10-19:
    /// "We do not need to show that every time"). Given at launch from where
    /// the app keeps it ([`told`](Self::told)); gone with the file's layer.
    told: Option<(PathBuf, String)>,
}

/// A layer that was refused: it did not parse, or it parsed and did not fit
/// what was already there. Nothing was applied.
///
/// **Deliberately not `Display`.** The error renders with the offending source
/// line quoted, which can be a secret, and every message a user is shown is
/// also logged — so the only way to say it is
/// [`Config::describe_parse_error`], which needs the text and drops the quote.
#[derive(Debug)]
pub struct Refused(pub toml::de::Error);

impl ConfigStack {
    /// **Launch**: the system file, then yours, then the `project`'s when
    /// the caller found one in a trusted start ([`project_path`]) — each a
    /// layer if it exists and parses — and the config they add up to, with
    /// everything the user must be told. Reads the environment; the rest is
    /// [`source`](Self::source) and [`config`](Self::config).
    ///
    /// `told`: what you were last told of the project file's safety settings,
    /// as the app kept it from [`told`](Self::told) — its note is said again
    /// only if the file now says something else, and then names what.
    pub fn load_reporting(
        project: Option<&Path>,
        told: Option<&str>,
    ) -> (Self, Config, Vec<String>) {
        let user = Config::config_path();
        if user.is_none() {
            warn!("neither XDG_CONFIG_HOME nor HOME set; no user config");
        }
        let (stack, config, mut said) = Self::load_from(
            Config::system_config_path().as_deref(),
            user.as_deref(),
            project,
            told,
        );
        if user.is_none() {
            said.insert(
                0,
                "no $XDG_CONFIG_HOME or $HOME — running without a user config".into(),
            );
        }
        (stack, config, said)
    }

    /// [`load_reporting`](Self::load_reporting) without the environment: the
    /// system file, THEN yours, then the project's, so the later wins where
    /// they say the same thing (rules stack across all three).
    ///
    /// What each file brought, then anything the result says that no file
    /// did — once each. The second half is only ever the defensive
    /// "could not be combined" note: every standing note was new at the layer
    /// that raised it. (Which also means a problem a later file cures is still
    /// reported: it is still in the file that has it.)
    pub(super) fn load_from(
        system: Option<&Path>,
        user: Option<&Path>,
        project: Option<&Path>,
        told: Option<&str>,
    ) -> (Self, Config, Vec<String>) {
        let mut stack = Self {
            told: project.zip(told).map(|(p, d)| (file_key(p), d.to_string())),
            ..Self::default()
        };
        let mut said = Vec::new();
        if let Some(system) = system {
            said.extend(stack.load_file(Origin::System, system));
        }
        if let Some(user) = user {
            said.extend(stack.load_file(Origin::User, user));
        }
        if let Some(project) = project {
            said.extend(stack.load_file(Origin::Project, project));
        }
        let (config, notes) = stack.config();
        for note in notes {
            if !said.contains(&note) {
                said.push(note);
            }
        }
        (stack, config, said)
    }

    /// **Read `path` and put it on the stack**, returning what the user must
    /// be told: whatever [`source`](Self::source) said about the layer, or why
    /// the file was left out (a missing one is silent: no config is a normal
    /// state).
    ///
    /// **`source`'s notes are passed on, never dropped** — they include the
    /// safety heads-up ([`Origin::is_yours`]), and a loader that kept only the
    /// stack's standing notes would lose it for any file it loads.
    ///
    /// A file that cannot be read or does not parse is left out, and says so
    /// loudly — the cost is stated, because running without your file means
    /// its `[secrets]` settings (your sensitive paths and names) are not in
    /// effect, which is less protective than you believe.
    fn load_file(&mut self, origin: Origin, path: &Path) -> Vec<String> {
        let what = match origin {
            Origin::System => format!("the system config ({})", path.display()),
            Origin::User => "your config".to_string(),
            Origin::Project => format!("the project's config ({})", path.display()),
            // A file is never the command line (`source` asserts it); named
            // by its path either way.
            Origin::Sourced | Origin::CommandLine | Origin::Environment | Origin::Tutor => {
                path.display().to_string()
            }
        };
        let text = match read_file(path) {
            Ok(Some(text)) => text,
            Ok(None) => {
                info!(path = %path.display(), "no config file there");
                return Vec::new();
            }
            Err(why) => {
                // `read_file`'s reasons are a path's KIND or an errno, never
                // file content: the read is what failed, so there is nothing
                // parsed to leak.
                warn!(path = %path.display(), error = %why, "could not read a config file");
                return vec![format!(
                    "{what} could not be READ ({why}) — running without it"
                )];
            }
        };
        match self.source(origin, path, &text) {
            Ok((_, notes)) => notes,
            Err(Refused(err)) => {
                // **The rendered error quotes the offending source line, and
                // that line can BE a secret.** `[[alias]] value = "…"` holds
                // literals, and a stray token after one makes it the line
                // `toml` points at — proved by
                // `a_parse_error_quotes_the_offending_line`. The screen used
                // to get the quote while the log got the safe form, but what
                // the screen gets is logged too (`App::post`), so both get
                // `describe_parse_error`: the line NUMBER, never the line.
                let said = Config::describe_parse_error(&err, &text);
                warn!(path = %path.display(), error = %said, "config has errors; running without it");
                // What running without it actually COSTS. There are no user
                // permission rules yet (TODO.md, "User-editable permission
                // policy"); the loss that is real is every `[secrets]` setting.
                vec![format!(
                    "{what} NOT LOADED — {said}. Running without it: its [secrets] \
                     settings (sensitive paths, names, redaction) are NOT in effect"
                )]
            }
        }
    }

    /// **Put `text`, read from `path`, on top of the stack** and return the
    /// config the stack now adds up to — or refuse it, changing nothing.
    ///
    /// Checked twice before anything is kept: the file on its own (so a typo
    /// is reported against the file that has it), and on top of the stack
    /// (so the running config always deserializes). The notes are the ones
    /// this layer brought: the file's own, any the new combination raised
    /// that the old one did not — never the stack's standing notes again —
    /// and, for a file that is not yours, the safety settings it changed.
    ///
    /// **Logged, with what it changed**: the log is where you find out which
    /// file set something, at launch as much as at `:source` (Steven: "make
    /// sure that in the logs we're showing what's being loaded"). Key names
    /// only — a value can be a secret (`secrets.extra`), and every `[secrets]`
    /// field is a scalar or a list, so a change names the field, never the
    /// contents.
    pub fn source(
        &mut self,
        origin: Origin,
        path: &Path,
        text: &str,
    ) -> Result<(Config, Vec<String>), Refused> {
        self.source_all(&[(origin, path, text)])
            .map_err(|(_, refused)| refused)
    }

    /// **Put several files on the stack as one change** — `:reload`'s:
    /// your config, then the project's — or refuse them all, changing
    /// nothing. Each is checked alone before any is kept, as
    /// [`source`](Self::source) checks one; `Err` names which refused (its
    /// index in `files`). The notes are every file's, in order.
    pub fn source_all(
        &mut self,
        files: &[(Origin, &Path, &str)],
    ) -> Result<(Config, Vec<String>), (usize, Refused)> {
        let mut layers = Vec::with_capacity(files.len());
        for (i, &(origin, path, text)) in files.iter().enumerate() {
            debug_assert!(
                !matches!(
                    origin,
                    Origin::CommandLine | Origin::Environment | Origin::Tutor
                ),
                "a file is never the command line, the environment or the tutor"
            );
            let refused = |e| (i, Refused(e));
            let table: toml::Table = toml::from_str(text).map_err(refused)?;
            // The file alone, through the same door as launch: a nested typo
            // is a `deny_unknown_fields` error HERE, naming this file's line.
            Config::parse(text).map_err(refused)?;
            layers.push(Layer {
                origin,
                path: Some(file_key(path)),
                table,
            });
        }
        let was = self.layers.clone();
        let mut said = Vec::new();
        for (i, layer) in layers.into_iter().enumerate() {
            match self.push(layer) {
                Ok((_, notes)) => said.extend(notes),
                // Unreachable as `push`'s own restore is (each file was
                // checked alone); kept so a later file cannot leave an
                // earlier one applied.
                Err(e) => {
                    self.layers = was;
                    return Err((i, Refused(e)));
                }
            }
        }
        Ok((self.config().0, said))
    }

    /// **The project's file, when one is loaded** — the start's
    /// `.shannon/config.toml`: what `:reload` reads again after yours, and
    /// `:untrust` takes back off.
    pub fn project_file(&self) -> Option<&Path> {
        self.layers
            .iter()
            .find(|layer| layer.origin == Origin::Project)?
            .path
            .as_deref()
    }

    /// **Put a `:set` on top of the stack** — a layer, as a file is, so a file
    /// sourced afterwards that names the same key wins: vim's `:source` runs
    /// after your `:set` and sets what it says, and `:reload` is a `:source`
    /// (Steven: `:reload` wins, "it is like re-sourcing a file").
    ///
    /// Checked alone first, as `source` checks a file: a value of the wrong
    /// type, or a key no section has, is refused with serde's own sentence,
    /// and nothing changes. The sentence may quote the value — never a
    /// credential, which `set::plan` refuses before it gets here.
    ///
    /// Consecutive `:set`s share one layer, as `push` merges any layer into a
    /// top one from the same source.
    pub fn set(&mut self, table: toml::Table) -> Result<(Config, Vec<String>), String> {
        let alone: Config = toml::Value::Table(table.clone())
            .try_into()
            .map_err(|e: toml::de::Error| e.message().to_string())?;
        // A top-level name no section has is CAPTURED by the schema, not
        // rejected (a newer release's section in an older binary) — which is
        // right for a file and wrong for something you just typed.
        if let Some(key) = alone.unknown.keys().next() {
            return Err(format!("unknown option: `{key}` — `:set all` lists them"));
        }
        self.push(Layer {
            origin: Origin::CommandLine,
            path: None,
            table,
        })
        .map_err(|e| e.message().to_string())
    }

    /// **`CLAUDE_CODE_EFFORT_LEVEL` sets the level at start** (Steven, 10-06)
    /// — laid where a `:set effort=` typed before anything else would be,
    /// under its own origin ([`Origin::Environment`]): over your files,
    /// under whatever is set or sourced later (a `:reload` whose file names
    /// `[turn] effort` included, as it outranks a `:set`).
    ///
    /// `value` is the variable as launch read it
    /// ([`EFFORT_VAR`](crate::backend::claude_code::EFFORT_VAR)), so no test
    /// reads the real one. `config` becomes what the stack adds up to. The
    /// sentence is what the user is told: the level taken, or the value
    /// ignored. Nothing for an unset or empty variable.
    pub fn start_effort(&mut self, config: &mut Config, value: Option<&OsStr>) -> Option<String> {
        use crate::backend::claude_code::EFFORT_VAR;
        let word = value?.to_string_lossy();
        let word = word.trim();
        if word.is_empty() {
            return None;
        }
        let Some(level) = crate::Effort::parse(word) else {
            let levels: Vec<&str> = crate::Effort::ALL.iter().map(|e| e.as_str()).collect();
            return Some(format!(
                "{EFFORT_VAR}=`{word}` is not a level ({}) — ignored, and not passed to Claude Code",
                levels.join(", ")
            ));
        };
        let over = config.turn.effort.filter(|was| *was != level);
        let turn = toml::Table::from_iter([("effort".to_string(), level.as_str().into())]);
        let table = toml::Table::from_iter([("turn".to_string(), toml::Value::Table(turn))]);
        // Its own origin, not a `:set`'s: a `:set` typed later merges into a
        // command-line layer, and `:config save` writes what was typed.
        let layer = Layer {
            origin: Origin::Environment,
            path: None,
            table,
        };
        match self.push(layer) {
            Ok((now, _)) => *config = now,
            // Unreachable while `[turn] effort` takes every level; said if not.
            Err(why) => return Some(format!("{EFFORT_VAR} not applied — {}", why.message())),
        }
        Some(match over {
            Some(was) => format!("effort {level} from {EFFORT_VAR}, over your config's {was}"),
            None => format!("effort {level} from {EFFORT_VAR}"),
        })
    }

    /// **`shannon tutor`'s settings, laid at launch** under their own origin
    /// ([`Origin::Tutor`]) — over a stack that holds no file, since the
    /// tutor reads none: what is in force is the defaults and these, then
    /// whatever is `:set` or `:source`d after. `Err` is the refusal, for a
    /// table no config takes (a launch's bug; the caller says it).
    pub fn start_tutor(&mut self, table: toml::Table) -> Result<Config, String> {
        self.push(Layer {
            origin: Origin::Tutor,
            path: None,
            table,
        })
        .map(|(config, _)| config)
        .map_err(|why| why.message().to_string())
    }

    /// **Lay `layer` on top and fold** — the end of both [`source`](Self::source)
    /// and [`set`](Self::set), so what a layer is told and what the log records
    /// cannot differ by how it arrived.
    ///
    /// The notes are the ones this layer brought: any the new combination
    /// raised that the old one did not — never the stack's standing notes
    /// again — and, for a layer that is not yours, the safety settings it
    /// changed.
    ///
    /// **Logged, with what it changed**: the log is where you find out which
    /// file set something, at launch as much as at `:source` (Steven: "make
    /// sure that in the logs we're showing what's being loaded"). Key names
    /// only — a value can be a secret (`secrets.extra`), and every `[secrets]`
    /// field is a scalar or a list, so a change names the field, never the
    /// contents.
    ///
    /// **A layer from the same source as the top one merges into it** — the
    /// same file again (`:reload`, a second `:source`), or another `:set`.
    /// Nothing is between them, so no key's value differs from stacking, and
    /// the stack grows by the files you load rather than by every reload: it
    /// used to gain a layer per `:reload`, and `:scriptnames` listed your
    /// config once per reload. (Vim lists a script once however often it is
    /// sourced.) A file sourced again with another on top of it still gets a
    /// layer of its own — it must win over the one between.
    fn push(&mut self, layer: Layer) -> Result<(Config, Vec<String>), toml::de::Error> {
        let (below, before) = self.config();
        let (origin, name) = (layer.origin, layer.name());
        let ignored = match origin.is_yours() {
            true => Vec::new(),
            false => yours_only_in(&layer.table),
        };
        let was = self.layers.clone();
        self.lay(layer);
        let mut config = match self.fold() {
            Ok(config) => config,
            // **Defensive, and unreachable today**: both callers check the
            // layer on its own first, and laying valid tables over each other
            // keeps every key's type, so this fold cannot fail. Kept so a
            // future merge rule (an array append, say) cannot leave a
            // half-applied layer — but no test can reach it (mutation-checked:
            // deleting the restore passes), so it is not counted as a guard.
            Err(e) => {
                self.layers = was;
                return Err(e);
            }
        };
        let mut new: Vec<String> = config
            .review()
            .into_iter()
            .filter(|n| !before.contains(n))
            .collect();
        let changed = below.changed_keys(&config);
        info!(
            from = %name,
            origin = origin.word(),
            changed = %changed.join(", "),
            "config layer loaded"
        );
        let touched = safety_touched(&changed);
        if !origin.is_yours()
            && !touched.is_empty()
            && let Some(telling) = self.tell(origin)
            && let Some(note) = safety_note(origin, &name, &touched, telling)
        {
            new.push(note);
        }
        if !ignored.is_empty() {
            // Not "where the model writes" — that is one of five
            // (review 2026-10-13, C).
            new.push(format!(
                "{name} sets {} — ignored: only your own config sets {}",
                ignored.join(", "),
                match ignored.len() {
                    1 => "it",
                    _ => "them",
                }
            ));
        }
        Ok((config, new))
    }

    /// **What of a layer's safety note is news — and it is said from here.**
    /// All of it for a file you `:source`, which you ask for each time. For the
    /// project's file, read at every launch: all of it at the trust that
    /// loads it (a fresh layer, nothing told), then only the settings whose
    /// digest ([`safety_digests`]) is not what you were last told — `None`
    /// when none is. Named, so a note said again does not read as the one you
    /// saw (review stop 6, F 3: after a pull added `allow rm *` it was the
    /// same sentence, byte for byte).
    fn tell(&mut self, origin: Origin) -> Option<Telling> {
        if origin != Origin::Project {
            return Some(Telling::Whole);
        }
        let Some(at) = self.layers.iter().position(|l| l.origin == origin) else {
            return Some(Telling::Whole);
        };
        // What it is laid over, too: the same file says something new when a
        // line of yours under it goes (review stop 6, B 2: your own
        // `approval_delay_ms = 0` hid the project's from the note; deleted,
        // the project's 0 ruled, unsaid).
        let below = ConfigStack {
            layers: self.layers[..at].to_vec(),
            told: None,
        };
        let layer = &self.layers[at];
        let below = below.config().0.as_table();
        let digests = safety_digests(&layer.table, &below);
        let now = (layer.path.clone()).map(|p| (p, told_token(&digests)));
        // What was told of THIS file; another's, or the one-digest token
        // written before (10-19, `ef9e3f8`), tells of nothing.
        let was: std::collections::HashMap<&str, &str> = match (&self.told, &now) {
            (Some((told_at, token)), Some((at, _))) if told_at == at => token
                .split(',')
                .filter_map(|pair| pair.split_once(':'))
                .collect(),
            _ => Default::default(),
        };
        let telling = match was.is_empty() {
            true => Some(Telling::Whole),
            false => {
                let since: Vec<String> = digests
                    .into_iter()
                    .filter(|d| was.get(d.key_hash.as_str()) != Some(&d.value_hash.as_str()))
                    .map(|d| d.key)
                    .collect();
                (!since.is_empty()).then_some(Telling::Since(since))
            }
        };
        self.told = now;
        telling
    }

    /// **The project's file and its safety settings' digests, as last said**
    /// — one token of hex, `:` and `,` (no space, no line break) — for the
    /// app to keep between runs and hand back to
    /// [`load_reporting`](Self::load_reporting). `None` while no project's
    /// file is loaded or nothing it says about safety has been said.
    pub fn told(&self) -> Option<(&Path, &str)> {
        self.told.as_ref().map(|(p, d)| (p.as_path(), d.as_str()))
    }

    /// **Where a layer goes** — on top, or into the top one when that is the
    /// same source ([`push`](Self::push)) — **except a project's file, which
    /// stands right above the latest read of your files**: the place launch
    /// gives it (system, yours, project — then the `:set`s), wherever it
    /// arrives from. The trust screen's yes, answered over a launch that had
    /// already set `CLAUDE_CODE_EFFORT_LEVEL`, put it on top, so the
    /// project's `[turn] effort` beat the variable a trusted launch keeps.
    ///
    /// Taken out before the rest is laid and put back after, so a `:reload`
    /// of yours merges into your layer under it rather than stacking a copy
    /// per reload, and the project follows it up. One project's file at a
    /// time: the start's.
    fn lay(&mut self, layer: Layer) {
        let project = self
            .layers
            .iter()
            .position(|l| l.origin == Origin::Project)
            .map(|i| self.layers.remove(i));
        let project = match (layer.origin, project) {
            (Origin::Project, Some(mut was)) if was.path == layer.path => {
                merge(&mut was.table, &layer.table);
                Some(was)
            }
            (Origin::Project, _) => Some(layer),
            (_, project) => {
                match self.layers.last_mut() {
                    Some(top) if top.origin == layer.origin && top.path == layer.path => {
                        merge(&mut top.table, &layer.table)
                    }
                    _ => self.layers.push(layer),
                }
                project
            }
        };
        if let Some(project) = project {
            let yours = self
                .layers
                .iter()
                .rposition(|l| matches!(l.origin, Origin::System | Origin::User));
            self.layers.insert(yours.map_or(0, |i| i + 1), project);
        }
    }

    /// **Every layer that sets `key`, top first** — its origin, its file (none
    /// for the command line), and the value IT gives. The first is the one in
    /// effect (before the config adjusts it); empty means the default holds.
    ///
    /// What `:verbose set x?` shows. Vim's answers "last set from", which for
    /// an option you just typed is the one thing you already know (Steven: it
    /// "might be better to just state the default or what it is set in the
    /// config") — the layers under it are what `:unsource`, `:set x&` or
    /// quitting would give you back.
    ///
    /// A `STACKED` list is listed only from each source's latest read —
    /// the earlier one adds nothing (`fold`), so naming it
    /// would show an entry that is not in force.
    pub fn setters(&self, key: &str) -> Vec<(Origin, Option<&Path>, &toml::Value)> {
        let stacked = STACKED.iter().any(|s| key_is_under(key, s));
        self.layers
            .iter()
            .enumerate()
            .rev()
            .filter(|&(i, _)| !(stacked && self.superseded(i)))
            .filter_map(|(_, layer)| {
                let value = super::set::get(&layer.table, key)?;
                Some((layer.origin, layer.path.as_deref(), value))
            })
            .collect()
    }

    /// **The `[permissions] rules` a project's file holds that no file of
    /// yours does** — what the gate names as the project's
    /// (`RuleSet::with_project_rules`). Files only: a `:set` of the list
    /// holds the whole list, and claims nothing.
    pub fn project_rules(&self) -> Vec<String> {
        let held = |origins: &[Origin]| -> Vec<String> {
            self.setters("permissions.rules")
                .into_iter()
                .filter(|(origin, ..)| origins.contains(origin))
                .filter_map(|(_, _, value)| value.as_array())
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        };
        let yours = held(&[Origin::System, Origin::User]);
        held(&[Origin::Project])
            .into_iter()
            .filter(|rule| !yours.contains(rule))
            .collect()
    }

    /// **This session's `:set`s, as a project's file would say them** —
    /// `:config save`'s options (Steven, 10-13): each key a `:set` named, at
    /// the value now in force (`config`). Not the environment's
    /// ([`Origin::Environment`]): nobody typed it.
    ///
    /// A list the files add to (`STACKED`) is never written whole: a `:set`
    /// holds the whole list in force — your files' entries, the defaults,
    /// the project's own — and written over the project's list it wiped the
    /// team's entries from it (review 2026-10-13, A). So it gives what the
    /// `:set`s added, and what they took out of the project's own entries;
    /// a `-=` of an entry another file gives is said, not written.
    pub fn typed_for_project(&self, config: &Config) -> Typed {
        let mut typed = toml::Table::new();
        for layer in self
            .layers
            .iter()
            .filter(|l| l.origin == Origin::CommandLine)
        {
            merge(&mut typed, &layer.table);
        }
        let as_table = |config: &Config| toml::Table::try_from(config).unwrap_or_default();
        let in_force = as_table(config);
        let files = ConfigStack {
            layers: (self.layers.iter())
                .filter(|l| l.origin != Origin::CommandLine)
                .cloned()
                .collect(),
            told: None,
        };
        let from_files = as_table(&files.config().0);
        let theirs = self
            .layers
            .iter()
            .find(|l| l.origin == Origin::Project)
            .map(|l| &l.table);
        let strings = |table: Option<&toml::Table>, path: &str| -> Vec<String> {
            table
                .and_then(|t| super::set::get(t, path))
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        };
        let mut out = Typed::default();
        for path in super::set::leaf_paths(&typed) {
            // By what it is under: `exec.env` is a table, its keys the leaves.
            if YOURS_ONLY.iter().any(|only| key_is_under(&path, only)) {
                out.left
                    .push(format!("{path} (only your own config sets it)"));
                continue;
            }
            if !STACKED.contains(&path.as_str()) {
                if let Some(value) = super::set::get(&in_force, &path) {
                    super::set::put(&mut out.values, &path, value.clone());
                }
                continue;
            }
            let now = strings(Some(&in_force), &path);
            let below = strings(Some(&from_files), &path);
            let project = strings(theirs, &path);
            let add: Vec<String> = now.iter().filter(|v| !below.contains(v)).cloned().collect();
            let mut take = Vec::new();
            for gone in below.iter().filter(|v| !now.contains(v)) {
                match project.contains(gone) {
                    true => take.push(gone.clone()),
                    false => out.left.push(format!(
                        "the `-=` of `{gone}` on {path} (not the project's entry, and a \
                         project's file only adds to that list)"
                    )),
                }
            }
            if !add.is_empty() || !take.is_empty() {
                out.lists.push(Listed { path, add, take });
            }
        }
        out
    }

    /// **Is the layer at `i` read again further up?** — the same source
    /// (your config `:reload`ed with a `:source`d file between). One answer for
    /// [`fold`](Self::fold) and [`setters`](Self::setters).
    fn superseded(&self, i: usize) -> bool {
        let layer = &self.layers[i];
        self.layers[i + 1..]
            .iter()
            .any(|later| later.origin == layer.origin && later.path == layer.path)
    }

    /// **Take every layer `path` added off the stack**, and return the config
    /// the rest adds up to — `None` when nothing from `path` is loaded.
    ///
    /// Every layer, not the last one: a file sourced twice is loaded twice,
    /// and "unload this file" means none of it is in effect afterwards.
    pub fn unload(&mut self, path: &Path) -> Option<(Config, Vec<String>)> {
        let key = file_key(path);
        let from = |layer: &Layer| layer.path.as_deref() == Some(key.as_path());
        if !self.layers.iter().any(from) {
            return None;
        }
        let (was, standing) = self.config();
        self.layers.retain(|layer| !from(layer));
        // What you were told of it goes with it: trusted again, it is said
        // again.
        if self.told.as_ref().is_some_and(|(told, _)| *told == key) {
            self.told = None;
        }
        let (config, mut notes) = self.config();
        // What going away RAISED, as `push` says what arriving raised — not
        // every note the stack still stands on, which `:unsource` used to say
        // all over again.
        notes.retain(|note| !standing.contains(note));
        // The log's other half: what going away gave back (see `source`).
        // `from`, as the load line names it, so one grep follows a file on
        // and off (Steven's log read that way on 2026-09-29).
        info!(
            from = %key.display(),
            changed = %was.changed_keys(&config).join(", "),
            "config layer unloaded"
        );
        Some((config, notes))
    }

    /// The config the stack adds up to, and what to say about it — every note,
    /// for launch. A stack that cannot fold (impossible for one built through
    /// `source`, which refuses such a layer) degrades to the defaults and says
    /// so rather than panicking.
    pub fn config(&self) -> (Config, Vec<String>) {
        match self.fold() {
            Ok(mut config) => {
                let notes = config.review();
                (config, notes)
            }
            Err(e) => (
                Config::default(),
                vec![format!(
                    "config could not be combined ({e}) — running on built-in defaults"
                )],
            ),
        }
    }

    /// The FILES loaded, bottom first — `:scriptnames`. `:set`s are not files
    /// and are not listed, as vim's list does not show the command line.
    pub fn layers(&self) -> impl Iterator<Item = (Origin, &Path)> {
        self.layers
            .iter()
            .filter_map(|l| Some((l.origin, l.path.as_deref()?)))
    }

    /// Every layer folded over the defaults, later keys winning — except a
    /// [`STACKED`] list, which every FILE adds to, each through its latest
    /// read only.
    ///
    /// A file can be on the stack twice (your config, `:reload`ed with a
    /// `:source`d file between): its earlier layer's other keys are simply
    /// overridden by the later one, but a stacked list would ADD its old
    /// entries back — an `allow` you deleted from your config and `:reload`ed
    /// away would stay in force. So only a source's last layer contributes
    /// its stacked lists.
    fn fold(&self) -> Result<Config, toml::de::Error> {
        let mut merged = toml::Table::new();
        let defaults = toml::Table::try_from(Config::default()).unwrap_or_default();
        // The stacked lists a file of yours names, in any order: then yours
        // replaces the default, wherever it stands — a `:reload` of your
        // config after a sourced file must not land yours on the default
        // that file's list was laid on.
        let named_by_yours: Vec<&str> = STACKED
            .iter()
            .copied()
            .filter(|path| {
                self.layers.iter().enumerate().any(|(i, layer)| {
                    layer.origin.is_yours() && !self.superseded(i) && names(&layer.table, path)
                })
            })
            .collect();
        for (i, layer) in self.layers.iter().enumerate() {
            let onto = Onto {
                superseded: self.superseded(i),
                yours: layer.origin.is_yours(),
                typed: layer.origin == Origin::CommandLine,
                defaults: &defaults,
                named_by_yours: &named_by_yours,
            };
            stack_onto(&mut merged, &layer.table, "", onto);
        }
        // **Yours first, on a list that stacks**: its order is precedence —
        // the first location wins a skill's name and leads the instructions —
        // and laying layers in load order put a sourced file's entries ahead
        // of yours after a `:reload` of your config (Reviewers A and C, round
        // 27). Your entries lead, in your files' order; the rest follow as
        // they were added.
        for path in &named_by_yours {
            let mut yours: Vec<toml::Value> = Vec::new();
            for (i, layer) in self.layers.iter().enumerate() {
                if !layer.origin.is_yours() || self.superseded(i) {
                    continue;
                }
                let items = super::set::get(&layer.table, path).and_then(toml::Value::as_array);
                for item in items.into_iter().flatten() {
                    if !yours.contains(item) {
                        yours.push(item.clone());
                    }
                }
            }
            if let Some(list) = list_at_mut(&mut merged, path) {
                // Only what is in force leads: an entry a `:set` took out
                // must not come back through your file's copy of it.
                yours.retain(|item| list.contains(item));
                let rest: Vec<toml::Value> = list
                    .drain(..)
                    .filter(|item| !yours.contains(item))
                    .collect();
                list.extend(yours);
                list.extend(rest);
            }
        }
        toml::Value::Table(merged).try_into()
    }
}

/// **The one spelling of a file the stack keys layers by**: canonical when it
/// exists (symlinks resolved, `.` and `..` gone, a case-blind volume's case
/// as on disk); when it does not, its nearest existing ancestor's canonical
/// spelling with the rest below it, as it will be once made.
///
/// Launch loads your config by one spelling, `:reload` and `:unsource` are
/// handed another, and a dotfiles setup commonly symlinks `~/.config` — so
/// comparing paths as given, `:unsource` could remove one of your config's
/// layers and leave another. Normalized here, on the way in AND on the way
/// out, so no caller has to remember to.
///
/// A file not made yet was keyed absolute, as typed: `../here/.shannon/
/// config.toml` and `/tmp/…` for `/private/tmp/…` were other files than
/// the project's, and `:config save <file>` wrote the project's config past
/// its trust gate (review stop 17, V3 M1). A `..` below what exists is kept
/// as written: there is nothing on disk to resolve it by.
fn file_key(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut below = Vec::new();
    let mut at = path.as_path();
    loop {
        if let Ok(real) = std::fs::canonicalize(at) {
            return below.iter().rev().fold(real, |key, part| key.join(part));
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                below.push(name.to_os_string());
                at = parent;
            }
            _ => return path,
        }
    }
}

/// **Are `a` and `b` the same file, as the stack sees files?** — by the one
/// spelling it keys layers by, so a caller asking "is this my config?" gets
/// the answer `:unsource` and `:scriptnames` would.
pub fn same_file(a: &Path, b: &Path) -> bool {
    file_key(a) == file_key(b)
}

/// The most a config file may hold: far past any real one (a large config is
/// a few KiB), far short of what hurts to hold in memory.
pub const MAX_CONFIG_BYTES: u64 = 1 << 20;

/// **Read a config file, or say why not — without blocking, and without
/// reading more than a config file can be.** `Ok(None)` when nothing is
/// there: a normal state for your config, an error for `:source` (the caller
/// says which).
///
/// `:source /dev/stdin` froze the app: reading a terminal or a pipe waits for
/// an end that never comes, on the thread that draws the screen. And
/// `:source /dev/zero` never ends at all — a `String` growing until memory
/// runs out. A config file is a small regular file, so this reads exactly
/// that and refuses the rest, each with a reason:
///
/// - opened NON-BLOCKING, because opening a FIFO waits for a writer, and then
///   judged by the handle it opened — so a path swapped between a check and
///   the open is judged as what was actually opened;
/// - a regular file only — not a directory, device, pipe or socket;
/// - at most [`MAX_CONFIG_BYTES`], counted WHILE reading, since a file can
///   grow after its size is known.
///
/// The one reader for launch's files, `:source`/`:reload`, `:config edit`'s
/// starting text and a `tui.theme` file (shannon-tui's `theme`), so none of
/// them can wait on what another refuses.
pub fn read_file(path: &Path) -> Result<Option<String>, String> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let kind = file.metadata().map_err(|e| e.to_string())?;
    if kind.is_dir() {
        return Err("it is a directory".into());
    }
    if !kind.is_file() {
        return Err("it is not a regular file — a device, a pipe or a socket".into());
    }
    let mut text = String::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() as u64 > MAX_CONFIG_BYTES {
        return Err(format!(
            "it is over {} KiB — larger than a config file",
            MAX_CONFIG_BYTES >> 10
        ));
    }
    Ok(Some(text))
}

/// The [`SAFETY_KEYS`] settings among a layer's `changed` keys.
fn safety_touched(changed: &[String]) -> Vec<&str> {
    changed
        .iter()
        .map(String::as_str)
        .filter(|key| SAFETY_KEYS.iter().any(|prefix| key_is_under(key, prefix)))
        .collect()
}

/// What a layer's safety note says ([`ConfigStack::tell`]).
#[derive(Debug, PartialEq, Eq)]
enum Telling {
    /// All it changes: nothing was told of the file, or it is one you
    /// `:source`.
    Whole,
    /// The settings changed since you were last told, by key.
    Since(Vec<String>),
}

/// **The heads-up a file not yours earns by changing a safety setting** —
/// which ones (`touched`, all of them or, said again, those changed since,
/// [`Telling`]), and how to take the file back out. `None` when nothing it
/// changes is news.
///
/// A note like any other: it still applies. The file is one you chose to
/// load, and local wins (Steven: "prefer local settings over global but also
/// serve a warning message").
fn safety_note(origin: Origin, name: &str, touched: &[&str], telling: Telling) -> Option<String> {
    // A project's file follows its trust (6d): `:untrust` is the verb that
    // keeps it off — after an `:unsource`, `:reload` reads it again.
    let undo = match origin {
        Origin::Project => "`:untrust` takes it back out",
        _ => "`:unsource` takes it back out",
    };
    match telling {
        Telling::Whole => Some(format!(
            "{name} changes safety settings: {} — {undo}",
            touched.join(", ")
        )),
        Telling::Since(since) => {
            let news: Vec<&str> = since
                .iter()
                .map(String::as_str)
                .filter(|key| touched.contains(key))
                .collect();
            let rules = match news.iter().any(|key| key_is_under(key, "permissions")) {
                true => " (`:rules` shows them)",
                false => "",
            };
            (!news.is_empty()).then(|| {
                format!(
                    "{name} changed safety settings since you were last told: {}{rules} — {undo}",
                    news.join(", ")
                )
            })
        }
    }
}

/// One safety setting's digest ([`safety_digests`]): its key, and a hash of
/// the key and of what it says.
struct SafetyDigest {
    key: String,
    key_hash: String,
    value_hash: String,
}

/// **What a file says about safety, over what it is laid on, setting by
/// setting** — each [`SAFETY_KEYS`] setting in `table` with its value and,
/// where it REPLACES the value beneath, the value `below` holds there — so
/// editing one (a rule a `git pull` brought) changes its digest, and so does
/// a line of yours beneath it going ([`ConfigStack::tell`]). A list that
/// STACKS ([`STACKED`]) adds to yours whatever lies beneath it, so your own
/// rules changing is not this file's news (review stop 6, F 3: a rule you
/// added to your own config said the project's note again).
///
/// Hashes, not the settings or even their keys: `secrets.extra` holds
/// credentials, and is named by them; this is kept in Shannon's state
/// directory, and as hex it can be no spelling that breaks the line it is
/// kept on.
fn safety_digests(table: &toml::Table, below: &toml::Table) -> Vec<SafetyDigest> {
    let hash = |text: &str| blake3::hash(text.as_bytes()).to_hex()[..32].to_string();
    super::set::leaf_paths(table)
        .into_iter()
        .filter(|key| SAFETY_KEYS.iter().any(|prefix| key_is_under(key, prefix)))
        .map(|key| {
            let stacks = STACKED.iter().any(|list| key_is_under(&key, list));
            let under = (!stacks).then(|| super::set::get(below, &key));
            // `Debug`, not TOML: it cannot fail, and it is stable for one value.
            let said = format!("{:?} over {under:?}", super::set::get(table, &key));
            SafetyDigest {
                key_hash: hash(&key),
                value_hash: hash(&said),
                key,
            }
        })
        .collect()
}

/// The digests as kept between runs ([`ConfigStack::told`]): `key:value`
/// hashes, comma-joined.
fn told_token(digests: &[SafetyDigest]) -> String {
    let pairs: Vec<String> = digests
        .iter()
        .map(|d| format!("{}:{}", d.key_hash, d.value_hash))
        .collect();
    pairs.join(",")
}

/// **The settings that are lists of independent statements, not one value**
/// — they STACK across files: a project's rules are added to yours rather
/// than replacing them (Steven, 2026-10-03: "rules should stack"; a conflict
/// is still decided strictest-wins, by the engine). Every other array is one
/// value, which a later file replaces, as `:set` replaces an option.
///
/// Across FILES only ([`ConfigStack::fold`]): a file read again merges into
/// its own layer by [`merge`], which replaces, so a rule deleted from your
/// config is gone after `:reload`.
const STACKED: &[&str] = &[
    "permissions.rules",
    "permissions.remove",
    // The locations of what the model is told (Steven, 10-06: "any config
    // file can specify/append memory and skill locations").
    "memory.instructions",
    "memory.paths",
    "skills.paths",
    // What is not read: a project's file may add to it (less read, never
    // more).
    "memory.disabled",
    "skills.disabled",
    "mcp.disabled",
    // What is not drawn: a project's file may add to it.
    "tui.alerts.disabled",
    // What is treated as a secret: a project's file may add caution, never
    // replace yours with its own (review stop 12, Q11, Steven 10-27). The
    // shipped credential seed applies whatever these say.
    "secrets.paths",
    "secrets.names",
    "secrets.prefixes",
    // Literal values to scrub: replaced, a project's file dropped yours —
    // caution taken away by the same move, so it stacks by the same rule.
    "secrets.extra",
    // What no command is handed: a project's file may add to it (more
    // withheld, never less).
    "exec.withhold",
];

/// **The settings only your own files set** — a file that is not yours
/// ([`Origin::is_yours`]) naming one is ignored there, and said so (`push`).
/// Where the model WRITES is the one: a project's config could otherwise
/// point the model's unasked writes at `~/.ssh` (Claude Code refuses a
/// repository's settings the same move: its settings reference, on
/// `autoMemoryDirectory`). So is how
/// `{project}` is named, which decides where that write place is, and what
/// marks the project's root, which decides what "the repository" is — and
/// so what a relative location may read. And what
/// is carved out of the credential paths: another's file could carve `~/.ssh`.
/// And `exec.env`: a project's `PATH` would decide what a name your rules
/// allow runs (`allow ls`, and its own `ls` first on the path). And the
/// three `[secrets]` lists that take caution AWAY (review stop 12, Q11,
/// Steven 10-27): `names_exclude` (stop masking `password`),
/// `value_delimiters` (replace the shipped set), `env` (lend your variables
/// as spendable secrets) — a trusted project's file said them at load, and
/// they applied. And `claude_code.env`: `inherit` hands Claude Code's
/// commands everything your shell holds, which `minimal` was chosen to stop;
/// and `claude_code.web` and `claude_code.path_policy`, whose only moves
/// from the default are looser — a fetch unasked, a credential path read —
/// where Claude Code's model sees what its tools return as it is (Steven,
/// 10-31, S18-Q4: "Go ahead").
const YOURS_ONLY: &[&str] = &[
    "memory.write",
    "memory.project",
    "memory.project_root",
    "secrets.paths_except",
    "secrets.names_exclude",
    "secrets.value_delimiters",
    "secrets.env",
    "exec.env",
    "claude_code.env",
    "claude_code.web",
    "claude_code.path_policy",
];

/// How one layer is laid on: whether its source reads it again further up
/// (`superseded`), whether it is yours, whether it is a `:set` (`typed`),
/// the compiled defaults a stacked list from a file that is not yours adds
/// to, and the stacked lists a file of yours names (which then replace the
/// default instead).
#[derive(Clone, Copy)]
struct Onto<'a> {
    superseded: bool,
    yours: bool,
    typed: bool,
    defaults: &'a toml::Table,
    named_by_yours: &'a [&'a str],
}

/// Lay one LAYER onto the layers below it: [`merge`], except that a
/// [`STACKED`] list is added to — entries already there are not repeated —
/// and, for a layer its own source has `superseded`, not contributed at all.
///
/// **Your list replaces the default; any other file's adds to it.** A
/// stacked list's first appearance used to stand in for the compiled
/// default whoever wrote it, so a project's `memory.paths` dropped your
/// global memory directory. Now a list from a file that is not yours lands on
/// the default when nothing above has named the key; yours replaces it, as
/// any setting you write does. (`permissions.rules`' default is empty: no
/// change there.) And a [`YOURS_ONLY`] key from a file that is not yours is
/// not laid on at all.
fn stack_onto(base: &mut toml::Table, over: &toml::Table, prefix: &str, onto: Onto) {
    for (key, value) in over {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        let stacked = STACKED.contains(&path.as_str());
        if !onto.yours && YOURS_ONLY.contains(&path.as_str()) {
            continue;
        }
        if stacked
            && !onto.yours
            && !base.contains_key(key)
            && !onto.named_by_yours.contains(&path.as_str())
            && let Some(default) = super::set::get(onto.defaults, &path)
        {
            base.insert(key.clone(), default.clone());
        }
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(below)), toml::Value::Table(above)) => {
                stack_onto(below, above, &path, onto);
            }
            // A table not there yet is still walked, not inserted whole: a
            // stacked list inside it must be judged like any other (a
            // superseded read's `[permissions]` carried its old rules in).
            (_, toml::Value::Table(above)) => {
                let mut fresh = toml::Table::new();
                stack_onto(&mut fresh, above, &path, onto);
                base.insert(key.clone(), toml::Value::Table(fresh));
            }
            _ if stacked && onto.superseded => {}
            // A `:set`'s list REPLACES the list below: `set::plan` built it
            // from the whole list in force, so it is the whole list — `x-=v`
            // takes `v` out wherever it came from, `x=…` sets it, as vim's
            // do. Added like a file's, a `-=` of an entry a file held put it
            // straight back (Steven, 10-07). A file read afterwards still
            // adds to it.
            _ if stacked && onto.typed => {
                base.insert(key.clone(), value.clone());
            }
            (Some(toml::Value::Array(below)), toml::Value::Array(above)) if stacked => {
                for item in above {
                    if !below.contains(item) {
                        below.push(item.clone());
                    }
                }
            }
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Does `table` name the dotted `path`? (`set::get`: the one reader of a
/// dotted key.)
fn names(table: &toml::Table, path: &str) -> bool {
    super::set::get(table, path).is_some()
}

/// The list at the dotted `path` in `table`, to reorder.
fn list_at_mut<'t>(table: &'t mut toml::Table, path: &str) -> Option<&'t mut Vec<toml::Value>> {
    let (sections, last) = path.rsplit_once('.').unwrap_or(("", path));
    let mut at = table;
    for section in sections.split('.').filter(|s| !s.is_empty()) {
        at = at.get_mut(section)?.as_table_mut()?;
    }
    at.get_mut(last)?.as_array_mut()
}

/// The [`YOURS_ONLY`] keys `table` names — what a file that is not yours is
/// told it may not set.
fn yours_only_in(table: &toml::Table) -> Vec<&'static str> {
    YOURS_ONLY
        .iter()
        .copied()
        .filter(|path| names(table, path))
        .collect()
}

/// Lay `over` onto `base`: tables merge key by key at every depth; any other
/// value replaces.
fn merge(base: &mut toml::Table, over: &toml::Table) {
    for (key, value) in over {
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(below)), toml::Value::Table(above)) => merge(below, above),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Every stacked list stacks, and every key that is yours alone is
    /// yours** — built from `STACKED` and `YOURS_ONLY` themselves, where
    /// per-key examples held a few: `memory.instructions` dropped from
    /// `STACKED` passed both crates, a project's instruction locations then
    /// replacing yours (review stop 12, T9 10). Each key read by its path,
    /// a value of the type its default has.
    #[test]
    fn every_stacked_list_stacks_and_every_yours_only_key_is_yours() {
        let file = |path: &str, value: &str| {
            let (section, key) = path.rsplit_once('.').expect("a dotted key");
            format!("[{section}]\n{key} = {value}\n")
        };
        let folded = |s: &ConfigStack| toml::Table::try_from(s.config().0).expect("serializes");
        let defaults = toml::Table::try_from(Config::default()).expect("serializes");
        fn arrays(table: &toml::Table, at: &str, out: &mut Vec<String>) {
            for (key, value) in table {
                let path = if at.is_empty() {
                    key.clone()
                } else {
                    format!("{at}.{key}")
                };
                match value {
                    toml::Value::Array(_) => out.push(path),
                    toml::Value::Table(t) => arrays(t, &path, out),
                    _ => {}
                }
            }
        }
        // **Every array, placed**: one that stacks, or one value a later file
        // replaces (`STACKED`'s doc). The loops below walk the lists, and so
        // cannot see an entry dropped from one — `memory.instructions` was —
        // so the arrays the config HAS are the denominator: one added, or
        // dropped from `STACKED`, fails here until it is placed.
        const REPLACED: &[&str] = &[
            "memory.project_root",  // what marks the root: yours only
            "secrets.paths_except", // a carve-out of the guards: yours only
            // Yours only since review stop 12's Q11 (they take caution away);
            // `paths`, `names`, `prefixes` stack (they add it).
            "secrets.env",
            "secrets.names_exclude",
            "secrets.value_delimiters",
            "tui.ctx_colors",   // a look
            "tui.theme_import", // a look
        ];
        let mut every = Vec::new();
        arrays(&defaults, "", &mut every);
        every.sort();
        let mut placed: Vec<String> = (STACKED.iter().chain(REPLACED))
            .map(|s| s.to_string())
            .collect();
        placed.sort();
        assert_eq!(
            placed, every,
            "every array stacks or is replaced, said which"
        );
        for &path in STACKED {
            let mut s = ConfigStack::default();
            s.source(Origin::User, Path::new("user"), &file(path, r#"["yours"]"#))
                .expect("yours");
            s.source(
                Origin::Sourced,
                Path::new("proj"),
                &file(path, r#"["theirs"]"#),
            )
            .expect("a project's");
            assert_eq!(
                super::super::set::get(&folded(&s), path),
                Some(&toml::Value::Array(vec!["yours".into(), "theirs".into()])),
                "{path}: yours, then a project's, added"
            );
        }
        for &path in YOURS_ONLY {
            // A value of the default's own type, so it reads as one — and
            // for a key that names a variant, the variant that is not the
            // default.
            let variants: &[(&str, [&str; 2])] = &[
                ("memory.project", ["hash", "claude-code"]),
                ("claude_code.env", ["inherit", "minimal"]),
                ("claude_code.web", ["ask", "allow"]),
                ("claude_code.path_policy", ["deny", "allow"]),
            ];
            let variant = variants.iter().find(|(p, _)| *p == path).map(|(_, v)| v);
            let of = |word: &str| match (super::super::set::get(&defaults, path), variant) {
                (Some(toml::Value::Array(_)), _) => format!("[\"{word}\"]"),
                (Some(toml::Value::Table(_)), _) => {
                    format!("{{ {} = \"1\" }}", word.to_uppercase())
                }
                (Some(toml::Value::String(default)), Some(names)) => {
                    let other = names.iter().find(|v| **v != default);
                    format!("\"{}\"", other.expect("two variants"))
                }
                _ => format!("\"{word}\""),
            };
            let mut s = ConfigStack::default();
            let (_, notes) = s
                .source(
                    Origin::Sourced,
                    Path::new("proj"),
                    &file(path, &of("theirs")),
                )
                .expect("a project's");
            assert_eq!(
                super::super::set::get(&folded(&s), path),
                super::super::set::get(&defaults, path),
                "{path}: a project's value is not taken"
            );
            assert!(
                notes
                    .iter()
                    .any(|n| n.contains(path) && n.contains("ignored")),
                "{path}: and it is said: {notes:?}"
            );
            let (_, notes) = s
                .source(Origin::User, Path::new("user"), &file(path, &of("yours")))
                .expect("yours");
            let want: toml::Value = toml::from_str::<toml::Table>(&format!("v = {}", of("yours")))
                .expect("a value")
                .remove("v")
                .expect("v");
            assert_eq!(
                super::super::set::get(&folded(&s), path),
                Some(&want),
                "{path}: yours is"
            );
            assert!(!notes.iter().any(|n| n.contains("ignored")), "{notes:?}");
        }
    }

    fn stack(files: &[(&str, &str)]) -> ConfigStack {
        let mut s = ConfigStack::default();
        for (path, text) in files {
            s.source(Origin::Sourced, Path::new(path), text)
                .expect("parses");
        }
        s
    }

    /// **Later wins, and only for what it names** — the sparse-diff rule,
    /// across files. A key the top file does not mention keeps the value from
    /// below it, not the default.
    #[test]
    fn a_later_layer_wins_only_for_what_it_names() {
        let s = stack(&[
            ("user", "[tui]\nscrolloff = 3\nsplitbelow = true\n"),
            ("proj", "[tui]\nscrolloff = 9\n"),
        ]);
        let (c, _) = s.config();
        assert_eq!(c.tui.scrolloff, 9, "the later file wins");
        assert!(
            c.tui.splitbelow,
            "a key the later file omits keeps the earlier value"
        );
    }

    /// Tables merge at depth: a project's `[keymap.normal]` adds a mapping to
    /// yours, it does not throw yours away.
    #[test]
    fn tables_merge_key_by_key() {
        let s = stack(&[
            ("user", "[keymap.normal]\nQ = \"enter-command\"\n"),
            ("proj", "[keymap.normal]\nZ = \"enter-command\"\n"),
        ]);
        let (c, _) = s.config();
        let normal = &c.keymap["normal"];
        assert!(
            normal.contains_key("Q") && normal.contains_key("Z"),
            "{normal:?}"
        );
    }

    /// **Faithful `:source` is additive**: take a key OUT of a file and source
    /// it again, and the running value stays — vim does not un-set what a file
    /// no longer says. (`:reload` is the same operation on your own file.)
    #[test]
    fn re_sourcing_a_file_without_a_key_keeps_its_running_value() {
        let mut s = stack(&[("user", "[tui]\nscrolloff = 3\n")]);
        s.source(Origin::User, Path::new("user"), "")
            .expect("parses");
        assert_eq!(
            s.config().0.tui.scrolloff,
            3,
            "still 3 — additive, as in vim"
        );
    }

    /// **Launch: the system file first, then yours** — yours wins where both
    /// say something, and what only the system file says still holds.
    #[test]
    fn at_launch_the_system_file_applies_first_and_yours_wins() {
        let dir = tempfile::tempdir().unwrap();
        let system = dir.path().join("system.toml");
        let user = dir.path().join("user.toml");
        std::fs::write(&system, "[tui]\nscrolloff = 1\nsplitbelow = true\n").unwrap();
        std::fs::write(&user, "[tui]\nscrolloff = 2\n").unwrap();
        let (stack, c, said) = ConfigStack::load_from(Some(&system), Some(&user), None, None);
        assert_eq!(c.tui.scrolloff, 2, "yours wins");
        assert!(c.tui.splitbelow, "what only the system file says holds");
        assert!(said.is_empty(), "{said:?}");
        let origins: Vec<Origin> = stack.layers().map(|(o, _)| o).collect();
        assert_eq!(origins, [Origin::System, Origin::User]);

        let (_, c, said) =
            ConfigStack::load_from(None, Some(&dir.path().join("absent")), None, None);
        assert_eq!(c, Config::default(), "no files: the defaults");
        assert!(
            said.is_empty(),
            "a missing config is a normal state: {said:?}"
        );
    }

    /// **At launch the project's file applies last** (Steven, 10-13: system,
    /// global, project): its settings win where both speak, its rules add to
    /// yours, what only your files set it does not, and a safety setting it
    /// changes is said.
    #[test]
    fn at_launch_the_projects_file_applies_last() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let project = project_path(&dir.path().join("proj"));
        std::fs::create_dir_all(project.parent().unwrap()).unwrap();
        std::fs::write(
            &user,
            "[tui]\nscrolloff = 2\n[permissions]\nrules = [\"deny curl *\"]\n",
        )
        .unwrap();
        std::fs::write(
            &project,
            "[tui]\nscrolloff = 5\n[memory]\nwrite = \"/srv/elsewhere\"\n\
             [permissions]\nrules = [\"allow cargo test *\", \"deny curl *\"]\n",
        )
        .unwrap();
        let (stack, c, said) = ConfigStack::load_from(None, Some(&user), Some(&project), None);
        assert_eq!(c.tui.scrolloff, 5, "the project's setting wins");
        assert_eq!(c.permissions.rules, ["deny curl *", "allow cargo test *"]);
        assert_ne!(
            c.memory.write, "/srv/elsewhere",
            "where memories are written is yours alone"
        );
        assert!(said.iter().any(|s| s.contains("permissions")), "{said:?}");
        let origins: Vec<Origin> = stack.layers().map(|(o, _)| o).collect();
        assert_eq!(origins, [Origin::User, Origin::Project]);
        assert_eq!(
            stack.project_rules(),
            ["allow cargo test *"],
            "named as its — a rule yours holds too stays yours"
        );
    }

    /// **`:reload` reads yours, then the project's** — so the project's
    /// settings still win where both speak, an edit to yours is taken, and
    /// the stack holds two files however often it runs (a reload of yours
    /// alone stacked a copy over the project per run).
    #[test]
    fn a_reload_reads_the_projects_file_after_yours() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let project = project_path(&dir.path().join("proj"));
        std::fs::create_dir_all(project.parent().unwrap()).unwrap();
        std::fs::write(&user, "[tui]\nscrolloff = 2\n").unwrap();
        std::fs::write(&project, "[tui]\nscrolloff = 5\nmouse = false\n").unwrap();
        let (mut s, c, _) = ConfigStack::load_from(None, Some(&user), Some(&project), None);
        assert_eq!(c.tui.scrolloff, 5, "premise: the project's over yours");
        assert!(!c.tui.mouse, "premise: the project's");
        assert_eq!(s.project_file(), Some(file_key(&project).as_path()));
        let mine = "[tui]\nscrolloff = 3\nsplitbelow = true\n";
        for _ in 0..3 {
            let (c, _) = s
                .source_all(&[
                    (Origin::User, &user, mine),
                    (Origin::Project, &project, "[tui]\nscrolloff = 5\n"),
                ])
                .unwrap();
            assert_eq!(c.tui.scrolloff, 5, "the project's, still");
            assert!(c.tui.splitbelow, "your edit, taken");
            // Additive, as `:reload` is: a key taken out of the project's
            // file keeps its running value (review 2026-10-13, A, C: a
            // re-read that replaced its layer passed every test).
            assert!(!c.tui.mouse, "kept until relaunch");
        }
        let origins: Vec<Origin> = s.layers().map(|(o, _)| o).collect();
        assert_eq!(origins, [Origin::User, Origin::Project]);
        // All or nothing: a project's file that no longer parses refuses the
        // reload, yours included — measured on a key only yours sets: the
        // project's own `scrolloff` would mask yours either way (review
        // 2026-10-13, C).
        assert!(s.config().0.tui.splitbelow, "premise: yours set it");
        let before = s.config().0;
        let refused = s.source_all(&[
            (Origin::User, &user, "[tui]\nsplitbelow = false\n"),
            (Origin::Project, &project, "[tui\n"),
        ]);
        assert!(matches!(refused, Err((1, _))), "the project's file refused");
        assert_eq!(s.config().0, before, "nothing changed");
    }

    /// **What `:config save` writes of your `:set`s** (Steven, 10-13): each
    /// key typed, at its value in force; never the environment's effort; of a
    /// list the files add to, only what was added (your entries stay yours);
    /// a `-=` and a key only your files set named instead of written.
    #[test]
    fn the_sets_a_projects_file_would_hold() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        std::fs::write(&user, "[skills]\npaths = [\"/srv/mine\"]\n").unwrap();
        let project = project_path(&dir.path().join("proj"));
        std::fs::create_dir_all(project.parent().unwrap()).unwrap();
        std::fs::write(&project, "[skills]\npaths = [\"/srv/proj\"]\n").unwrap();
        let (mut s, mut c, _) = ConfigStack::load_from(None, Some(&user), Some(&project), None);
        s.start_effort(&mut c, Some(std::ffi::OsStr::new("high")));
        let set = |s: &mut ConfigStack, text: &str| s.set(toml::from_str(text).unwrap()).unwrap().0;
        set(&mut s, "[tui]\nscrolloff = 4\n");
        // A `+=`: the whole list in force, yours and the project's with it.
        set(
            &mut s,
            "[skills]\npaths = [\"/srv/mine\", \"/srv/proj\", \"/srv/team\"]\n",
        );
        set(&mut s, "[exec.env]\nFOO = \"bar\"\n");
        let c = set(&mut s, "[memory]\nwrite = \"/srv/w\"\n");
        let typed = s.typed_for_project(&c);
        let values: toml::Table = toml::from_str("[tui]\nscrolloff = 4\n").unwrap();
        assert_eq!(typed.values, values, "never the effort: {typed:?}");
        assert_eq!(
            typed.lists,
            [Listed {
                path: "skills.paths".into(),
                add: vec!["/srv/team".into()],
                take: Vec::new(),
            }],
            "what was added, never the whole list"
        );
        for only in ["memory.write", "exec.env.FOO"] {
            assert!(
                typed.left.iter().any(|l| l.starts_with(only)),
                "{only}: {:?}",
                typed.left
            );
        }
        // A `-=` of the project's entry takes it out of its file; of yours,
        // it is said.
        let c = set(&mut s, "[skills]\npaths = [\"/srv/team\"]\n");
        let typed = s.typed_for_project(&c);
        assert_eq!(
            typed.lists,
            [Listed {
                path: "skills.paths".into(),
                add: vec!["/srv/team".into()],
                take: vec!["/srv/proj".into()],
            }]
        );
        assert!(
            typed
                .left
                .iter()
                .any(|l| l.contains("`-=` of `/srv/mine` on skills.paths")),
            "{:?}",
            typed.left
        );
    }

    /// **A project's file arriving after launch stands where launch puts
    /// it** — right above yours, under what launch laid after it — so a yes
    /// on the trust screen gives what a trusted launch gives:
    /// `CLAUDE_CODE_EFFORT_LEVEL` (its own layer at launch) beat the
    /// project's `[turn] effort` there, and lost to it when the project went
    /// on top.
    #[test]
    fn a_projects_file_arriving_late_stands_where_launch_puts_it() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        std::fs::write(&user, "[tui]\nscrolloff = 2\n").unwrap();
        let (mut s, mut c, _) = ConfigStack::load_from(None, Some(&user), None, None);
        // As launch lays it — the environment's own layer, not a `:set`
        // (review 2026-10-13, C: a `:set` here left that layer untested).
        s.start_effort(&mut c, Some(std::ffi::OsStr::new("high")));
        let project = project_path(&dir.path().join("proj"));
        let (c, _) = s
            .source(
                Origin::Project,
                &project,
                "[tui]\nscrolloff = 5\n[turn]\neffort = \"low\"\n",
            )
            .unwrap();
        assert_eq!(c.tui.scrolloff, 5, "over yours");
        assert_eq!(
            c.turn.effort,
            crate::Effort::parse("high"),
            "under the :set typed before it"
        );
        let origins: Vec<Origin> = s.layers().map(|(o, _)| o).collect();
        assert_eq!(origins, [Origin::User, Origin::Project]);
    }

    /// Two spellings of one file are one file: `./p` and `p` load and unload
    /// as the same layer set.
    #[test]
    fn a_file_is_one_file_however_it_is_spelled() {
        let mut s = stack(&[("./spelled", "[tui]\nscrolloff = 9\n")]);
        s.source(
            Origin::Sourced,
            Path::new("spelled"),
            "[tui]\nsplitbelow = true\n",
        )
        .expect("parses");
        let (c, _) = s.unload(Path::new("spelled")).expect("loaded");
        assert_eq!(
            c.tui.scrolloff,
            Config::default().tui.scrolloff,
            "both layers went"
        );
        assert!(!c.tui.splitbelow);
    }

    /// **A file reached through a symlink is the file** — a dotfiles setup
    /// commonly links `~/.config` — so `:unsource` by either spelling takes
    /// out what `:source` by the other put in.
    #[cfg(unix)]
    #[test]
    fn a_file_is_one_file_through_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("proj.toml"), "").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let (via_real, via_link) = (real.join("proj.toml"), link.join("proj.toml"));

        for (on, off) in [(&via_link, &via_real), (&via_real, &via_link)] {
            let mut s = ConfigStack::default();
            s.source(Origin::Sourced, on, "[tui]\nscrolloff = 9\n")
                .expect("parses");
            assert!(same_file(on, off), "premise");
            let (c, _) = s.unload(off).expect("the same file, by its other spelling");
            assert_eq!(c.tui.scrolloff, Config::default().tui.scrolloff);
            assert_eq!(s.layers().count(), 0);
        }
    }

    /// **A `:set` is yours**: typing a safety setting yourself is not a file
    /// someone else wrote, so it earns no heads-up — while a sourced file that
    /// does the same does (the premise).
    #[test]
    fn a_set_of_a_safety_setting_is_yours_and_says_nothing() {
        let mut s = ConfigStack::default();
        let (c, notes) = s
            .set(typed("nosecrets.redact", &Config::default()))
            .expect("fits");
        assert!(!c.secrets.redact, "premise: it applied");
        assert!(safety(&notes).is_empty(), "{notes:?}");

        let (_, notes) = ConfigStack::default()
            .source(
                Origin::Sourced,
                Path::new("p"),
                "[secrets]\nredact = false\n",
            )
            .expect("parses");
        assert_eq!(safety(&notes).len(), 1, "a sourced file does: {notes:?}");
    }

    /// **Unloading takes every layer a file added**, and its keys fall back to
    /// what remains — the layer below, not the default, and a LATER layer
    /// keeps its say.
    #[test]
    fn unloading_a_file_falls_back_to_what_remains() {
        let mut s = stack(&[
            ("user", "[tui]\nscrolloff = 3\n"),
            ("proj", "[tui]\nscrolloff = 9\nsplitbelow = true\n"),
            ("proj", "[tui]\nsplitright = true\n"),
            ("late", "[tui]\nsplitbelow = false\n"),
        ]);
        let (c, _) = s.unload(Path::new("proj")).expect("proj was loaded");
        assert_eq!(c.tui.scrolloff, 3, "back to the layer below");
        assert!(!c.tui.splitright, "BOTH of proj's layers are gone");
        assert!(!c.tui.splitbelow, "a later layer's say is untouched");
        let proj = file_key(Path::new("proj"));
        assert!(
            s.layers().all(|(_, p)| p != proj),
            "no layer of proj's is left"
        );
        assert!(
            s.unload(Path::new("proj")).is_none(),
            "nothing left to unload"
        );
    }

    /// A file that does not parse, or names a key that does not exist, is
    /// refused whole, and the stack is exactly as it was.
    #[test]
    fn a_bad_file_is_refused_and_changes_nothing() {
        let mut s = stack(&[("user", "[tui]\nscrolloff = 3\n")]);
        for bad in [
            "[tui\n",
            "[tui]\nscrolof = 9\n",
            "[tui]\nscrolloff = \"nine\"\n",
        ] {
            let Err(Refused(e)) = s.source(Origin::Sourced, Path::new("bad"), bad) else {
                panic!("accepted: {bad:?}");
            };
            // The file is checked ON ITS OWN first, so the refusal can point
            // into it; the combined stack has no source text to point into.
            let said = Config::describe_parse_error(&e, bad);
            assert!(said.starts_with("line "), "names the file's line: {said}");
        }
        assert_eq!(s.layers().count(), 1, "no half-applied layer left behind");
        assert_eq!(s.config().0.tui.scrolloff, 3);
    }

    /// Sourcing reports what THIS file brought, not the stack's standing notes
    /// again — an unknown key in your config is said once, at launch, not at
    /// every later `:source`.
    #[test]
    fn sourcing_reports_only_the_new_notes() {
        let mut s = stack(&[("user", "future_thing = 1\n")]);
        assert_eq!(s.config().1.len(), 1, "premise: the standing note");
        let (_, notes) = s
            .source(Origin::Sourced, Path::new("p"), "[tui]\nscrolloff = 2\n")
            .expect("parses");
        assert!(notes.is_empty(), "{notes:?}");
        let (_, notes) = s
            .source(Origin::Sourced, Path::new("q"), "other_thing = 2\n")
            .expect("parses");
        assert_eq!(notes.len(), 1, "the new unknown key is said: {notes:?}");
    }

    /// The safety heads-up among a layer's notes, if any.
    fn safety(notes: &[String]) -> Vec<&String> {
        notes
            .iter()
            .filter(|n| n.contains("safety settings"))
            .collect()
    }

    /// **A file that is not yours says which safety settings it changed** —
    /// only those, only when they changed, and never for your own file. It
    /// still applies (local wins); the note is the "gentle" half.
    #[test]
    fn a_file_not_yours_names_the_safety_settings_it_changed() {
        let mut s = ConfigStack::default();
        let (c, notes) = s
            .source(
                Origin::Sourced,
                Path::new("proj"),
                "[secrets]\nredact = false\n[tui]\nscrolloff = 2\n",
            )
            .expect("parses");
        assert!(!c.secrets.redact, "premise: it applied");
        let said = safety(&notes);
        assert_eq!(said.len(), 1, "{notes:?}");
        assert!(
            said[0].contains("secrets.redact") && !said[0].contains("tui.scrolloff"),
            "names the safety setting, and only it: {}",
            said[0]
        );

        let (_, notes) = s
            .source(
                Origin::Sourced,
                Path::new("again"),
                "[secrets]\nredact = false\n",
            )
            .expect("parses");
        assert!(
            safety(&notes).is_empty(),
            "already off: a setting that did not change is not news: {notes:?}"
        );

        let (_, notes) = s
            .source(
                Origin::User,
                Path::new("mine"),
                "[exec]\nshell = \"zsh\"\n[agents]\nmax_depth = 9\n",
            )
            .expect("parses");
        assert!(
            safety(&notes).is_empty(),
            "your own file is not called out: {notes:?}"
        );

        // A single key, and an array of tables, as well as a section.
        let (_, notes) = s
            .source(
                Origin::Sourced,
                Path::new("more"),
                "[tui]\nconfirm_delete = false\n\n[[alias]]\nname = \"db\"\nvalue = \"x\"\n",
            )
            .expect("parses");
        let said = safety(&notes);
        assert!(
            said.len() == 1 && said[0].contains("alias, tui.confirm_delete"),
            "{notes:?}"
        );
    }

    /// **Loading a file at launch passes on what sourcing it said** — the
    /// safety note included. The loader used to keep only the stack's standing
    /// notes, which would have dropped it for any file loaded that way.
    #[test]
    fn loading_a_file_passes_on_its_safety_note() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proj.toml");
        std::fs::write(&path, "[exec]\naudit = \"off\"\n").unwrap();
        let said = ConfigStack::default().load_file(Origin::Sourced, &path);
        let said = safety(&said);
        assert!(
            said.len() == 1 && said[0].contains("exec.audit"),
            "{said:?}"
        );
    }

    /// **A project's safety note is said when it is news, not at every
    /// launch** (Steven, 10-19: "only the first time on enabling trust") —
    /// and again when what the file says about safety changes, or when it
    /// was taken off (`:untrust`) and comes back. Any other edit is quiet.
    #[test]
    fn a_projects_safety_note_is_said_when_it_is_news() {
        let dir = tempfile::tempdir().unwrap();
        let project = project_path(&dir.path().join("proj"));
        std::fs::create_dir_all(project.parent().unwrap()).unwrap();
        let rules = "[permissions]\nrules = [\"allow cargo test *\"]\n";
        std::fs::write(&project, rules).unwrap();
        let launch = |told: Option<&str>| ConfigStack::load_from(None, None, Some(&project), told);
        let (first, _, said) = launch(None);
        assert_eq!(safety(&said).len(), 1, "nothing told yet: {said:?}");
        let (path, digest) = first.told().expect("kept for the app");
        assert_eq!(path, file_key(&project));
        let digest = digest.to_string();

        let (again, _, said) = launch(Some(&digest));
        assert_eq!(safety(&said), Vec::<&String>::new(), "told already");
        assert_eq!(again.told().map(|(_, d)| d), Some(digest.as_str()));

        std::fs::write(&project, format!("{rules}[tui]\nscrolloff = 4\n")).unwrap();
        let (_, _, said) = launch(Some(&digest));
        assert_eq!(safety(&said), Vec::<&String>::new(), "not a safety edit");

        let gained = "[permissions]\nrules = [\"allow cargo test *\", \"allow rm *\"]\n";
        std::fs::write(&project, gained).unwrap();
        let (mut changed, _, said) = launch(Some(&digest));
        let said = safety(&said);
        assert!(
            said.len() == 1
                && said[0].contains(
                    "changed safety settings since you were last told: \
                     permissions.rules"
                ),
            "a rule gained since, named: {said:?}"
        );
        assert_ne!(changed.told().map(|(_, d)| d), Some(digest.as_str()));

        changed.unload(&project).expect("loaded");
        assert_eq!(changed.told(), None, "forgotten with its layer");
        let (_, said) = changed
            .source(Origin::Project, &project, gained)
            .expect("parses");
        assert_eq!(safety(&said).len(), 1, "trusted again: {said:?}");
    }

    /// **A line of yours under the project's going is news too** (review
    /// stop 6, B 2, probed): your `tui.approval_delay_ms = 0` hid the
    /// project's own 0 from its note; delete yours, and the project's 0
    /// rules — said at the next launch, not hidden behind a digest of the
    /// project's file alone.
    #[test]
    fn your_line_under_the_projects_going_is_news() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let project = project_path(&dir.path().join("proj"));
        std::fs::create_dir_all(project.parent().unwrap()).unwrap();
        let theirs = "[tui]\napproval_delay_ms = 0\n[permissions]\nrules = [\"allow ls *\"]\n";
        std::fs::write(&project, theirs).unwrap();
        std::fs::write(&user, "[tui]\napproval_delay_ms = 0\n").unwrap();
        let launch =
            |told: Option<&str>| ConfigStack::load_from(None, Some(&user), Some(&project), told);
        let (first, _, said) = launch(None);
        let note = safety(&said)
            .first()
            .map(|n| n.to_string())
            .unwrap_or_default();
        assert!(
            !note.contains("approval_delay_ms"),
            "premise, yours hid it: {note}"
        );
        let digest = first.told().map(|(_, d)| d.to_string()).expect("told");

        let (_, _, said) = launch(Some(&digest));
        assert_eq!(safety(&said), Vec::<&String>::new(), "nothing new");

        std::fs::write(&user, "[tui]\nscrolloff = 3\n").unwrap();
        let (_, config, said) = launch(Some(&digest));
        assert_eq!(
            config.tui.approval_delay_ms, 0,
            "premise: the project's rules"
        );
        let said = safety(&said);
        assert!(
            said.len() == 1 && said[0].contains("tui.approval_delay_ms"),
            "said: {said:?}"
        );
    }

    /// **A note said again names what changed, and only the project's news**
    /// (review stop 6, F 3): said again after a pull, it read exactly as the
    /// first, byte for byte — "I have seen this" — and a rule you added to
    /// your OWN config said it again, as though the project's had changed.
    /// Rules stack, so what lies beneath them is not the project's news. And
    /// a token kept before per-setting digests (one hash) tells of nothing.
    #[test]
    fn a_note_said_again_names_what_changed_and_only_the_projects() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let project = project_path(&dir.path().join("proj"));
        std::fs::create_dir_all(project.parent().unwrap()).unwrap();
        let theirs = "[tui]\napproval_delay_ms = 100\n[permissions]\nrules = [\"allow ls *\"]\n";
        std::fs::write(&project, theirs).unwrap();
        std::fs::write(&user, "[permissions]\nrules = [\"ask git push *\"]\n").unwrap();
        let launch =
            |told: Option<&str>| ConfigStack::load_from(None, Some(&user), Some(&project), told);
        let (first, _, said) = launch(None);
        let whole = safety(&said)
            .first()
            .map(|n| n.to_string())
            .unwrap_or_default();
        assert!(
            whole.contains("changes safety settings:")
                && whole.contains("permissions.rules")
                && whole.contains("tui.approval_delay_ms"),
            "premise, the whole note: {whole}"
        );
        let token = first.told().map(|(_, d)| d.to_string()).expect("told");

        let mine = "[permissions]\nrules = [\"ask git push *\", \"deny rm -rf *\"]\n";
        std::fs::write(&user, mine).unwrap();
        let (_, _, said) = launch(Some(&token));
        assert_eq!(safety(&said), Vec::<&String>::new(), "a rule of yours");

        let pulled = theirs.replace("\"allow ls *\"", "\"allow ls *\", \"allow rm *\"");
        std::fs::write(&project, pulled).unwrap();
        let (_, _, said) = launch(Some(&token));
        let again = safety(&said);
        assert!(
            again.len() == 1
                && again[0]
                    .contains("since you were last told: permissions.rules (`:rules` shows them)")
                && !again[0].contains("approval_delay_ms"),
            "the pulled rule, alone: {again:?}"
        );

        // A setting brought back to what lies beneath it changed its digest,
        // and changes nothing: not news (the rules, still the project's, are
        // as told).
        let (now, _, _) = launch(Some(&token));
        let token = now.told().map(|(_, d)| d.to_string()).expect("told");
        let pulled = std::fs::read_to_string(&project).unwrap();
        std::fs::write(&project, pulled.replace("= 100", "= 300")).unwrap();
        let (_, config, said) = launch(Some(&token));
        assert_eq!(config.tui.approval_delay_ms, 300, "premise: the default");
        assert_eq!(safety(&said), Vec::<&String>::new(), "back to yours");

        let before = blake3::hash(b"one digest").to_hex().to_string();
        let (_, _, said) = launch(Some(&before));
        let said = safety(&said);
        assert!(
            said.len() == 1 && said[0].contains("changes safety settings:"),
            "the old token: the whole note: {said:?}"
        );
    }

    /// **A project that turns your alerts back on wins, and says so**
    /// (review stop 9, E1; Steven, 10-24: "The project file should win but
    /// we can surface conflicts") — your `notify = false` under a trusted
    /// project's `true`: the project's holds, and the note names it; so
    /// does an alert of its own that notifies. A setting it shares with
    /// yours is no conflict, and named nowhere.
    #[test]
    fn a_project_turning_your_alerts_on_wins_and_is_said() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let project = project_path(&dir.path().join("proj"));
        std::fs::create_dir_all(project.parent().unwrap()).unwrap();
        std::fs::write(&user, "[tui.alerts]\nnotify = false\nbell = true\n").unwrap();
        let theirs = "[tui.alerts]\nnotify = true\nbell = true\n\
                      [tui.alert.built]\nwhen = \"running\"\nnotify = \"built\"\n";
        std::fs::write(&project, theirs).unwrap();
        let (_, config, said) = ConfigStack::load_from(None, Some(&user), Some(&project), None);
        assert!(config.tui.alerts.notify, "the project's file wins");
        let note = (safety(&said).first())
            .map(|n| n.to_string())
            .unwrap_or_default();
        assert!(
            note.contains("tui.alerts.notify") && note.contains("tui.alert.built"),
            "{note}"
        );
        assert!(
            !note.contains("tui.alerts.bell"),
            "as yours: no conflict: {note}"
        );
    }

    /// A `:set` as a stack layer: the table `set::plan` would make.
    fn typed(line: &str, now: &Config) -> toml::Table {
        super::super::set::plan(super::super::set::parse(line).unwrap(), now)
            .unwrap()
            .table
    }

    /// **A `:set` wins until a file that names the same key is sourced again**
    /// — `:reload` included (Steven: "it is like re-sourcing a file") — and a
    /// key that file does not name keeps what you typed.
    #[test]
    fn a_later_file_wins_over_a_set_and_only_for_what_it_names() {
        let mut s = stack(&[("user", "[tui]\nscrolloff = 3\n")]);
        let (now, _) = s.config();
        let (c, _) = s
            .set(typed("tui.scrolloff=7 tui.splitbelow", &now))
            .expect("fits");
        assert_eq!(c.tui.scrolloff, 7, "the :set wins over the file below it");
        assert!(c.tui.splitbelow);

        let (c, _) = s
            .source(Origin::User, Path::new("user"), "[tui]\nscrolloff = 3\n")
            .expect("parses");
        assert_eq!(c.tui.scrolloff, 3, ":reload wins over the earlier :set");
        assert!(
            c.tui.splitbelow,
            "a key the file does not name keeps the :set"
        );
    }

    /// **`CLAUDE_CODE_EFFORT_LEVEL` is a `:set effort=` at start** (Steven,
    /// 10-06): over the file, under a later `:set`, and a `:reload` naming
    /// `[turn] effort` wins as over any `:set`. A value that is not a level
    /// is said and changes nothing; unset or empty, nothing at all.
    #[test]
    fn the_effort_variable_is_a_set_at_start() {
        use crate::Effort;
        let var = |v: &str| Some(std::ffi::OsString::from(v));
        let file = "[turn]\neffort = \"high\"\n";
        let user = |s: &mut ConfigStack| s.source(Origin::User, Path::new("user"), file);

        let mut s = ConfigStack::default();
        user(&mut s).expect("parses");
        let (mut c, _) = s.config();
        let said = s.start_effort(&mut c, var(" XHigh ").as_deref());
        assert_eq!(c.turn.effort, Some(Effort::Xhigh), "over the file");
        assert_eq!(c.turn.effort, s.config().0.turn.effort, "the stack agrees");
        assert_eq!(
            said.as_deref(),
            Some("effort xhigh from CLAUDE_CODE_EFFORT_LEVEL, over your config's high")
        );
        let (c, _) = s.set(typed("effort=low", &c)).expect("fits");
        assert_eq!(c.turn.effort, Some(Effort::Low), "a later :set wins");
        let (c, _) = user(&mut s).expect("parses");
        assert_eq!(c.turn.effort, Some(Effort::High), ":reload wins");

        // No file, or the file's own level: no "over".
        let mut s = ConfigStack::default();
        let mut c = Config::default();
        let said = s.start_effort(&mut c, var("max").as_deref());
        assert_eq!(
            said.as_deref(),
            Some("effort max from CLAUDE_CODE_EFFORT_LEVEL")
        );
        let mut s = ConfigStack::default();
        user(&mut s).expect("parses");
        let (mut c, _) = s.config();
        let said = s.start_effort(&mut c, var("high").as_deref());
        assert_eq!(
            said.as_deref(),
            Some("effort high from CLAUDE_CODE_EFFORT_LEVEL")
        );

        for (value, held) in [(None, true), (var(""), true), (var("ultracode"), false)] {
            let mut s = ConfigStack::default();
            user(&mut s).expect("parses");
            let (mut c, _) = s.config();
            let said = s.start_effort(&mut c, value.as_deref());
            assert_eq!(
                c.turn.effort,
                Some(Effort::High),
                "{value:?} changes nothing"
            );
            assert_eq!(s.layers.len(), 1, "{value:?} adds no layer");
            assert_eq!(said.is_none(), held, "{value:?}: {said:?}");
        }
        let mut c = Config::default();
        let said = ConfigStack::default().start_effort(&mut c, var("ultracode").as_deref());
        assert!(
            said.is_some_and(|s| s.contains("`ultracode` is not a level")
                && s.contains("not passed to Claude Code")),
        );
    }

    /// **The tutor's settings are a layer nobody typed** (`shannon tutor`):
    /// in force over the defaults, under a later `:set`, named by `:verbose
    /// set`'s origin, and never in what `:config save` writes. A table no
    /// config takes is refused and lays nothing.
    #[test]
    fn the_tutors_settings_are_a_layer_nobody_typed() {
        let table: toml::Table =
            toml::from_str("[tui]\nmodel = \"stub\"\n[tui.prompt]\nheight = 3\n").unwrap();
        let mut s = ConfigStack::default();
        let c = s.start_tutor(table).expect("fits");
        assert_eq!((c.tui.model.as_str(), c.tui.prompt.height), ("stub", 3));
        assert_eq!(s.setters("tui.model")[0].0, Origin::Tutor);
        let (c, _) = s.set(typed("tui.prompt.height=5", &c)).expect("fits");
        assert_eq!(c.tui.prompt.height, 5, "a later :set wins");
        let saved = s.typed_for_project(&c).values;
        assert!(
            saved
                .get("tui")
                .is_some_and(|tui| tui.get("model").is_none()),
            "{saved:?}"
        );

        let mut s = ConfigStack::default();
        let wrong: toml::Table = toml::from_str("[tui]\nprompt = 3\n").unwrap();
        assert!(s.start_tutor(wrong).is_err());
        assert!(s.layers.is_empty(), "a refusal lays nothing");
    }

    /// **Your location list replaces the default; a project's adds to it**
    /// (Steven, 10-06: "any config file can specify/append memory and skill
    /// locations") — in whichever order the files were read: a `:reload` of
    /// yours after the project's must not land yours on the default. And
    /// where the model WRITES is yours alone, the other file told so.
    #[test]
    fn a_location_list_is_yours_to_replace_and_a_projects_to_add_to() {
        let project = "[memory]\npaths = [\"/shared/mem\"]\n[skills]\npaths = [\"/shared/sk\"]\n";
        let yours = "[memory]\npaths = [\"/mine\"]\n";
        let mut s = ConfigStack::default();
        s.source(Origin::Sourced, Path::new("proj"), project)
            .unwrap();
        let (c, _) = s.config();
        let mut want = Config::default().memory.paths;
        want.push("/shared/mem".into());
        assert_eq!(c.memory.paths, want, "a project's alone: onto the default");
        let mut want = Config::default().skills.paths;
        want.push("/shared/sk".into());
        assert_eq!(c.skills.paths, want);

        let mut s = ConfigStack::default();
        s.source(Origin::User, Path::new("user"), yours).unwrap();
        s.source(Origin::Sourced, Path::new("proj"), project)
            .unwrap();
        assert_eq!(s.config().0.memory.paths, ["/mine", "/shared/mem"]);
        s.source(Origin::User, Path::new("user"), yours).unwrap();
        // In order: the first location wins a skill's name, so a `:reload`
        // must not put the project's first (Reviewers A and C, round 27).
        assert_eq!(
            s.config().0.memory.paths,
            ["/mine", "/shared/mem"],
            "no default under yours, and yours first"
        );

        let mut s = ConfigStack::default();
        let (c, notes) = s
            .source(
                Origin::Sourced,
                Path::new("proj"),
                "[memory]\nwrite = \"/x/.ssh\"\n",
            )
            .unwrap();
        assert_eq!(c.memory.write, Config::default().memory.write);
        assert!(
            notes
                .iter()
                .any(|n| n.contains("memory.write") && n.contains("ignored")),
            "{notes:?}"
        );
        let (c, notes) = s
            .source(
                Origin::User,
                Path::new("user"),
                "[memory]\nwrite = \"/mine/w\"\n",
            )
            .unwrap();
        assert_eq!(c.memory.write, "/mine/w");
        assert!(!notes.iter().any(|n| n.contains("ignored")), "{notes:?}");
    }

    /// **What is carved out of the credential paths is yours alone** — a
    /// sourced file carving `~/.ssh` would make a key an ordinary file; and
    /// yours replaces the default, so `[]` puts `~/.claude` back whole.
    #[test]
    fn a_carve_out_of_the_credential_paths_is_yours_alone() {
        let mut s = ConfigStack::default();
        let (c, notes) = s
            .source(
                Origin::Sourced,
                Path::new("proj"),
                "[secrets]\npaths_except = [\"~/.ssh\"]\n",
            )
            .unwrap();
        assert_eq!(
            c.secrets.paths_except,
            Config::default().secrets.paths_except
        );
        assert!(
            notes
                .iter()
                .any(|n| n.contains("secrets.paths_except") && n.contains("ignored")),
            "{notes:?}"
        );
        let (c, _) = s
            .source(
                Origin::User,
                Path::new("user"),
                "[secrets]\npaths_except = []\n",
            )
            .unwrap();
        assert!(c.secrets.paths_except.is_empty());
    }

    /// **What Claude Code's calls may do is yours alone** — a project's file
    /// cannot let its web tools run unasked, its credential paths be read,
    /// or its environment be your whole shell again (Steven, 10-31,
    /// S18-Q4); your own file can.
    #[test]
    fn claude_codes_loosening_knobs_are_yours_alone() {
        use crate::config::{EnvPolicy, WebPolicy};
        let mut s = ConfigStack::default();
        let mine = "[claude_code]\nenv = \"minimal\"\n";
        s.source(Origin::User, Path::new("user"), mine).unwrap();
        let theirs = "[claude_code]\nweb = \"allow\"\npath_policy = \"allow\"\nenv = \"inherit\"\n";
        let (c, notes) = s
            .source(Origin::Sourced, Path::new("proj"), theirs)
            .unwrap();
        assert_eq!(c.claude_code.web, WebPolicy::Ask);
        assert_eq!(c.claude_code.path_policy, crate::PathPolicy::Deny);
        assert_eq!(c.claude_code.env, EnvPolicy::Minimal, "yours kept");
        for key in [
            "claude_code.web",
            "claude_code.path_policy",
            "claude_code.env",
        ] {
            assert!(
                notes
                    .iter()
                    .any(|n| n.contains(key) && n.contains("ignored")),
                "{key}: {notes:?}"
            );
        }
        let (c, _) = s.source(Origin::User, Path::new("user"), theirs).unwrap();
        assert_eq!(c.claude_code.web, WebPolicy::Allow, "yours, taken");
    }

    /// **How `{project}` is named is yours alone** — it decides where the
    /// model's writes go, as `memory.write` does.
    #[test]
    fn how_the_project_is_named_is_yours_alone() {
        use crate::ProjectName;
        let mut s = ConfigStack::default();
        let set = "[memory]\nproject = \"claude-code\"\n";
        let (c, notes) = s.source(Origin::Sourced, Path::new("proj"), set).unwrap();
        assert_eq!(c.memory.project, ProjectName::Hash);
        assert!(
            notes
                .iter()
                .any(|n| n.contains("memory.project") && n.contains("ignored")),
            "{notes:?}"
        );
        let (c, _) = s.source(Origin::User, Path::new("user"), set).unwrap();
        assert_eq!(c.memory.project, ProjectName::ClaudeCode);
    }

    /// **What marks the project's root is yours alone** — it decides what
    /// "the repository" is, and so what a relative location may read: a
    /// project's file naming a marker found at `/` would walk to `/`.
    #[test]
    fn what_marks_the_project_root_is_yours_alone() {
        let mut s = ConfigStack::default();
        let set = "[memory]\nproject_root = [\"bin\"]\n";
        let (c, notes) = s.source(Origin::Sourced, Path::new("proj"), set).unwrap();
        assert_eq!(c.memory.project_root, Config::default().memory.project_root);
        assert!(
            notes
                .iter()
                .any(|n| n.contains("memory.project_root") && n.contains("ignored")),
            "{notes:?}"
        );
        let (c, _) = s.source(Origin::User, Path::new("user"), set).unwrap();
        assert_eq!(c.memory.project_root, ["bin"]);
    }

    /// **What is disabled STACKS** — a project's file adds to your list (less
    /// read, never more), and cannot take an entry of yours away: every
    /// `disabled` list, `[mcp]`'s too (review 29, Reviewer C: H4).
    #[test]
    fn what_is_disabled_stacks() {
        let mut s = ConfigStack::default();
        s.source(
            Origin::User,
            Path::new("user"),
            "[skills]\ndisabled = [\"review\"]\n[memory]\ndisabled = [\"~/a.md\"]\n\
             [mcp]\ndisabled = [\"github\"]\n[tui.alerts]\ndisabled = [\"queued\"]\n",
        )
        .unwrap();
        let (c, _) = s
            .source(
                Origin::Sourced,
                Path::new("proj"),
                "[skills]\ndisabled = [\"ship\"]\n[memory]\ndisabled = [\"~/b.md\"]\n\
                 [mcp]\ndisabled = [\"slack\"]\n[tui.alerts]\ndisabled = [\"running\"]\n",
            )
            .unwrap();
        assert_eq!(c.skills.disabled, ["review", "ship"]);
        assert_eq!(c.memory.disabled, ["~/a.md", "~/b.md"]);
        assert_eq!(c.mcp.disabled, ["github", "slack"]);
        assert_eq!(c.tui.alerts.disabled, ["queued", "running"]);
    }

    /// **An alert of yours lays over the shipped ones, key by key** — a file
    /// naming one alert of its own keeps the five, a table with a shipped
    /// one's name changes only what it names, and `:set` reaches a shipped
    /// alert's key, which a file never had to write. The config is folded
    /// from the files alone, so a map they name was theirs wholesale.
    #[test]
    fn an_alert_of_yours_lays_over_the_shipped_ones() {
        let shipped = Config::default().tui.alert;
        assert_eq!(shipped.len(), crate::config::SHIPPED_ALERTS.len());
        let mut s = ConfigStack::default();
        let (c, _) = s
            .source(
                Origin::User,
                Path::new("user"),
                "[tui.alert.ctx]\nwhen = \"ctx_percent >= 80\"\nsay = \"hot\"\n\
                 border = \"ui.caution\"\n[tui.alert.decide]\nwhen = \"decision\"\n",
            )
            .unwrap();
        let names: Vec<&str> = c.tui.alert.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            [
                "ctx",
                "decide",
                "hidden",
                "interrupt",
                "queued",
                "running",
                "unshown",
                "unshown_session"
            ]
        );
        let decide = &c.tui.alert["decide"];
        assert_eq!(decide.when.as_deref(), Some("decision"));
        assert_eq!(decide.say, shipped["decide"].say, "the shipped say kept");
        assert_eq!(c.tui.alert["queued"], shipped["queued"]);
        assert_eq!(c.tui.alert["ctx"].border.as_deref(), Some("ui.caution"));
        let (c, _) = s
            .set(typed("tui.alert.queued.say=waiting", &c))
            .expect("fits");
        assert_eq!(c.tui.alert["queued"].say.as_deref(), Some("waiting"));
        assert_eq!(
            c.tui.alert["ctx"].say.as_deref(),
            Some("hot"),
            "a file's kept"
        );
    }

    /// **`:set x-=v` takes `v` out of a list that stacks, wherever it came
    /// from** (Steven, 10-07: "scan over the array and remove v") — a file
    /// of yours, a project's, a `:set` — and `x=` sets the whole list, as
    /// vim's do; a file read afterwards (a `:reload`) adds its entries back,
    /// as re-sourcing it would.
    #[test]
    fn a_typed_minus_takes_an_entry_out_wherever_it_came_from() {
        use crate::config::set;
        let mut s = ConfigStack::default();
        s.source(
            Origin::User,
            Path::new("user"),
            "[skills]\ndisabled = [\"a\", \"b\"]\n",
        )
        .unwrap();
        let (c, _) = s
            .source(
                Origin::Sourced,
                Path::new("proj"),
                "[skills]\ndisabled = [\"p\"]\n",
            )
            .unwrap();
        assert_eq!(c.skills.disabled, ["a", "b", "p"], "premise");
        let mut typed = |c: &Config, line: &str| {
            let plan = set::plan(set::parse(line).unwrap(), c).unwrap();
            s.set(plan.table).unwrap().0
        };
        let c = typed(&c, "skills.disabled-=a");
        assert_eq!(c.skills.disabled, ["b", "p"], "your file's entry, out");
        let c = typed(&c, "skills.disabled-=p");
        assert_eq!(c.skills.disabled, ["b"], "a project's, out");
        let c = typed(&c, "skills.disabled+=q");
        assert_eq!(c.skills.disabled, ["b", "q"]);
        let c = typed(&c, "skills.disabled=z");
        assert_eq!(c.skills.disabled, ["z"], "`=` sets the whole list");
        let (c, _) = s
            .source(
                Origin::User,
                Path::new("user"),
                "[skills]\ndisabled = [\"a\", \"b\"]\n",
            )
            .unwrap();
        let mut held = c.skills.disabled.clone();
        held.sort();
        assert_eq!(held, ["a", "b", "z"], "a reload adds its own back");
    }

    /// **`exec.env` is yours alone** — a project's `PATH` would decide what
    /// a name your rules allow runs.
    #[test]
    fn the_commands_environment_is_yours_alone() {
        let mut s = ConfigStack::default();
        let set = "[exec]\nenv = { PATH = \"./bin:/usr/bin\" }\n";
        let (c, notes) = s.source(Origin::Sourced, Path::new("proj"), set).unwrap();
        assert!(c.exec.env.is_empty());
        assert!(
            notes
                .iter()
                .any(|n| n.contains("exec.env") && n.contains("ignored")),
            "{notes:?}"
        );
        let (c, _) = s.source(Origin::User, Path::new("user"), set).unwrap();
        assert_eq!(
            c.exec.env.get("PATH").map(String::as_str),
            Some("./bin:/usr/bin")
        );
    }

    /// Consecutive `:set`s are one layer (nothing can come between them, so
    /// merging changes no value); a file between them keeps them apart.
    #[test]
    fn consecutive_sets_share_a_layer() {
        let mut s = ConfigStack::default();
        let now = Config::default();
        s.set(typed("tui.scrolloff=1", &now)).expect("fits");
        s.set(typed("tui.scrolloff=2 tui.splitright", &now))
            .expect("fits");
        assert_eq!(s.layers.len(), 1, "one layer: {:?}", s.layers);
        let (c, _) = s.config();
        assert_eq!(c.tui.scrolloff, 2, "the later :set wins inside it");
        assert!(c.tui.splitright);

        s.source(Origin::Sourced, Path::new("mid"), "[tui]\nscrolloff = 5\n")
            .expect("parses");
        s.set(typed("tui.splitbelow", &now)).expect("fits");
        assert_eq!(s.layers.len(), 3, "a file keeps the two runs apart");
        assert_eq!(s.config().0.tui.scrolloff, 5, "and its value stands");
    }

    /// **A `:set` the schema refuses changes nothing** — a word that is not
    /// one of an option's values, or a name no section has — and says why.
    #[test]
    fn a_set_the_schema_refuses_changes_nothing() {
        let mut s = stack(&[("user", "[tui]\nscrolloff = 3\n")]);
        let now = s.config().0;
        let mut bad = toml::Table::new();
        bad.insert(
            "tui".into(),
            toml::Value::Table(toml::Table::from_iter([(
                "resume".to_string(),
                toml::Value::String("sometimes".into()),
            )])),
        );
        let why = s.set(bad).expect_err("not a resume mode");
        assert!(why.contains("sometimes"), "serde's sentence: {why}");
        let why = s
            .set(
                typed("tui.graph_glyphs.child=x", &now)
                    .into_iter()
                    .chain([("nowhere".to_string(), toml::Value::Integer(1))])
                    .collect(),
            )
            .expect_err("no such section");
        assert!(why.contains("unknown option"), "{why}");
        let why = s
            .set(typed("tui.nothing=1", &now))
            .expect_err("tui has no such key");
        assert!(why.contains("nothing"), "{why}");
        assert_eq!(s.layers.len(), 1, "nothing was left on the stack");
        assert_eq!(s.config().0, now);
    }

    /// `:verbose set x?`'s answer: every layer naming the key, top first, each
    /// with the value IT gives; none for a default. `:scriptnames` lists files
    /// only.
    #[test]
    fn setters_are_every_layer_top_first_and_files_alone_are_scripts() {
        let mut s = stack(&[("user", "[tui]\nscrolloff = 3\n")]);
        let now = s.config().0;
        s.set(typed("tui.splitbelow tui.scrolloff=9", &now))
            .expect("fits");
        let user = file_key(Path::new("user"));
        assert_eq!(
            s.setters("tui.scrolloff"),
            [
                (Origin::CommandLine, None, &toml::Value::Integer(9)),
                (
                    Origin::Sourced,
                    Some(user.as_path()),
                    &toml::Value::Integer(3)
                ),
            ]
        );
        assert_eq!(
            s.setters("tui.splitbelow"),
            [(Origin::CommandLine, None, &toml::Value::Boolean(true))]
        );
        assert!(s.setters("tui.mouse").is_empty(), "the default");
        assert_eq!(s.layers().count(), 1, "the command line is not a script");
        assert!(
            s.unload(Path::new("user")).is_some(),
            "unloading the file leaves the :set"
        );
        assert!(s.config().0.tui.splitbelow);
    }

    /// **The reader takes a small regular file and refuses everything else,
    /// with a reason, without blocking.** `:source /dev/stdin` froze the app
    /// and `/dev/zero` would have eaten memory.
    #[test]
    fn the_reader_refuses_what_is_not_a_small_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_file(&dir.path().join("absent")), Ok(None));
        let why = read_file(dir.path()).expect_err("a directory");
        assert!(why.contains("directory"), "{why}");

        let at_cap = dir.path().join("at_cap.toml");
        std::fs::write(&at_cap, "#".repeat(MAX_CONFIG_BYTES as usize)).unwrap();
        assert!(
            matches!(read_file(&at_cap), Ok(Some(_))),
            "the cap itself is fine"
        );
        // Over the cap, with bytes that are not UTF-8 PAST it: the reason is
        // the size, which proves the read stopped at the cap rather than
        // reading the whole file and judging it after.
        let over = dir.path().join("over.toml");
        let mut bytes = "#".repeat(MAX_CONFIG_BYTES as usize + 1).into_bytes();
        bytes.extend([0xff; 16]);
        std::fs::write(&over, bytes).unwrap();
        let why = read_file(&over).expect_err("over the cap");
        assert!(why.contains("KiB"), "{why}");

        let binary = dir.path().join("binary.toml");
        std::fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
        assert!(read_file(&binary).is_err(), "not UTF-8");
    }

    /// A device and a pipe are refused — and the pipe without WAITING: opening
    /// a FIFO blocks until a writer comes, which would freeze the app on the
    /// thread that draws it. Read on a thread with a deadline, so a regression
    /// fails this test instead of hanging the suite.
    #[cfg(unix)]
    #[test]
    fn the_reader_never_waits_on_a_device_or_a_pipe() {
        let why = read_file(Path::new("/dev/zero")).expect_err("a device");
        assert!(why.contains("not a regular file"), "{why}");

        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path and a mode; no memory is shared.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let reading = fifo.clone();
        std::thread::spawn(move || tx.send(read_file(&reading)));
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(result) => {
                let why = result.expect_err("a pipe");
                assert!(why.contains("not a regular file"), "{why}");
            }
            Err(_) => {
                // Release the blocked reader so the thread can end, then fail.
                let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
                panic!("the reader BLOCKED on a FIFO");
            }
        }
    }

    /// **Launch runs without a config it cannot read, and says why** — a
    /// directory where the file should be is not a silent "no config".
    #[test]
    fn at_launch_an_unreadable_config_is_said_with_its_reason() {
        let dir = tempfile::tempdir().unwrap();
        let (_, c, said) = ConfigStack::load_from(None, Some(dir.path()), None, None);
        assert_eq!(c, Config::default());
        assert!(
            said.len() == 1 && said[0].contains("could not be READ (it is a directory)"),
            "{said:?}"
        );
    }

    /// **The same file again, directly on top, is one layer** — `:reload`
    /// used to add a layer each time, and `:scriptnames` listed your config
    /// once per reload. With another file between, it is a new layer, since
    /// it must win over that one.
    #[test]
    fn the_same_file_again_on_top_is_one_layer() {
        let mut s = stack(&[("user", "[tui]\nscrolloff = 3\n")]);
        for text in ["[tui]\nscrolloff = 4\n", "[tui]\nsplitbelow = true\n"] {
            s.source(Origin::Sourced, Path::new("user"), text)
                .expect("parses");
        }
        assert_eq!(s.layers().count(), 1, "one layer: {:?}", s.layers);
        let (c, _) = s.config();
        assert_eq!(c.tui.scrolloff, 4, "the later text wins");
        assert!(c.tui.splitbelow, "and it is still additive");

        s.source(Origin::Sourced, Path::new("proj"), "[tui]\nscrolloff = 9\n")
            .expect("parses");
        s.source(Origin::Sourced, Path::new("user"), "[tui]\nscrolloff = 5\n")
            .expect("parses");
        assert_eq!(s.layers().count(), 3, "a file between keeps them apart");
        assert_eq!(s.config().0.tui.scrolloff, 5, "and the top one wins");
        let (c, _) = s.unload(Path::new("user")).expect("loaded");
        assert_eq!(c.tui.scrolloff, 9, "unloading takes both of user's layers");
    }

    /// **A file not made yet is one file however it is spelt** — through
    /// `..`, through a link to the directory it will be in, with `.` and `//`
    /// — keyed by what exists above it, as it will be once made (review stop
    /// 17, V3 M1: `:config save ../here/.shannon/config.toml` was another file
    /// than the project's). Two such files stay two.
    #[test]
    fn a_file_not_made_yet_is_one_file_however_spelt() {
        let dir = tempfile::TempDir::new().unwrap();
        let here = dir.path().join("here");
        std::fs::create_dir(&here).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&here, &link).unwrap();
        let file = here.join(".shannon/config.toml");
        assert!(!file.exists(), "premise: not made");
        for spelt in [
            here.join("../here/.shannon/config.toml"),
            link.join(".shannon/config.toml"),
            here.join("./.shannon//config.toml"),
        ] {
            assert!(same_file(&spelt, &file), "{spelt:?}");
        }
        assert!(!same_file(&here.join(".shannon/other.toml"), &file));
        assert!(!same_file(&dir.path().join(".shannon/config.toml"), &file));
    }

    /// **`:unsource` says what going away raised, not every standing note
    /// again** — an unknown key in your config was re-said at every unload.
    #[test]
    fn unloading_says_only_the_new_notes() {
        let mut s = stack(&[
            ("user", "future_thing = 1\n"),
            ("proj", "[tui]\nscrolloff = 2\n"),
        ]);
        assert_eq!(s.config().1.len(), 1, "premise: the standing note");
        let (_, notes) = s.unload(Path::new("proj")).expect("loaded");
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// Launch says each thing once: what a file brought and what the result
    /// says overlap, and the overlap is not repeated.
    #[test]
    fn at_launch_a_note_is_said_once() {
        let dir = tempfile::tempdir().unwrap();
        let system = dir.path().join("system.toml");
        std::fs::write(&system, "future_thing = 1\n").unwrap();
        let (_, _, said) = ConfigStack::load_from(Some(&system), None, None, None);
        assert_eq!(
            said.iter().filter(|n| n.contains("future_thing")).count(),
            1,
            "{said:?}"
        );
    }

    /// The rules each file's `[permissions]` names.
    fn rules(s: &ConfigStack) -> Vec<String> {
        s.config().0.permissions.rules
    }

    /// **Rules stack across files** (Steven, 2026-10-03: "rules should
    /// stack") — a project's are added to yours, not swapped for them; a rule
    /// both name is in force once; `remove` stacks the same way; and taking a
    /// file off takes its rules with it.
    #[test]
    fn rules_stack_across_files() {
        let mut s = ConfigStack::default();
        s.source(
            Origin::User,
            Path::new("mine"),
            "[permissions]\nrules = [\"deny rm:-r\"]\nremove = [\"ask find:-delete\"]\n",
        )
        .expect("parses");
        s.source(
            Origin::Sourced,
            Path::new("proj"),
            "[permissions]\nrules = [\"allow ls *\", \"deny rm:-r\"]\n\
             remove = [\"ask find:-exec\"]\n",
        )
        .expect("parses");
        assert_eq!(rules(&s), ["deny rm:-r", "allow ls *"], "added, once each");
        assert_eq!(
            s.config().0.permissions.remove,
            ["ask find:-delete", "ask find:-exec"]
        );

        s.unload(Path::new("proj")).expect("loaded");
        assert_eq!(rules(&s), ["deny rm:-r"], "the project's went with it");
    }

    /// **A rule deleted from a file is gone once the file is read again** —
    /// whether the new read merges into the file's own layer or lands above
    /// another file. Stacking every layer would have added the OLD read's
    /// rules back: an `allow` you deleted and `:reload`ed away, still in force.
    #[test]
    fn a_file_counts_only_through_its_latest_read() {
        let v1 = "[permissions]\nrules = [\"allow curl *\", \"deny rm:-r\"]\n";
        let v2 = "[permissions]\nrules = [\"deny rm:-r\"]\n";

        let mut s = ConfigStack::default();
        s.source(Origin::User, Path::new("mine"), v1)
            .expect("parses");
        s.source(Origin::User, Path::new("mine"), v2)
            .expect("parses");
        assert_eq!(rules(&s), ["deny rm:-r"], "re-read in place");

        let mut s = ConfigStack::default();
        s.source(Origin::User, Path::new("mine"), v1)
            .expect("parses");
        s.source(
            Origin::Sourced,
            Path::new("proj"),
            "[permissions]\nrules = [\"allow ls *\"]\n",
        )
        .expect("parses");
        s.source(Origin::User, Path::new("mine"), v2)
            .expect("parses");
        assert_eq!(
            s.layers.len(),
            3,
            "premise: a file between, so two layers of mine"
        );
        // Yours first, as every stacked list orders (`fold`).
        assert_eq!(
            rules(&s),
            ["deny rm:-r", "allow ls *"],
            "the deleted `allow curl *` is not back"
        );
        let shown = s.setters("permissions.rules");
        assert_eq!(
            shown.len(),
            2,
            "the superseded read is not named: {shown:?}"
        );
    }

    /// **A project's file adds secret-caution and never takes it away**
    /// (review stop 12, Q11, Steven 10-27) — by what each key DOES, since the
    /// generic test above follows the lists and would pass a key moved from
    /// one to another: `names_exclude = ["password"]` (stop masking it),
    /// `value_delimiters`, `env` from a project are not taken; its `names`,
    /// `paths`, `prefixes`, `extra` are added to yours.
    #[test]
    fn a_projects_secrets_add_caution_and_never_take_it_away() {
        let mut s = ConfigStack::default();
        s.source(
            Origin::User,
            Path::new("user"),
            "[secrets]\nnames = [\"acme\"]\npaths = [\"~/mine\"]\n\
             prefixes = [\"mine_\"]\nextra = [\"mine-value\"]\n",
        )
        .expect("yours");
        s.source(
            Origin::Sourced,
            Path::new("proj"),
            "[secrets]\nnames = [\"theirs\"]\npaths = [\"~/theirs\"]\n\
             prefixes = [\"theirs_\"]\nextra = [\"their-value\"]\n\
             names_exclude = [\"password\"]\nvalue_delimiters = [\" \"]\n\
             env = [\"AWS_SECRET_ACCESS_KEY\"]\n",
        )
        .expect("a project's");
        let secrets = s.config().0.secrets.clone();
        let fresh = Config::default().secrets;
        assert_eq!(secrets.names_exclude, fresh.names_exclude, "masking kept");
        assert_eq!(secrets.value_delimiters, fresh.value_delimiters);
        assert!(secrets.env.is_empty(), "nothing lent by a project");
        assert_eq!(secrets.names, ["acme", "theirs"]);
        assert_eq!(secrets.paths, ["~/mine", "~/theirs"]);
        assert_eq!(secrets.prefixes, ["mine_", "theirs_"]);
        assert_eq!(secrets.extra, ["mine-value", "their-value"]);
    }

    /// Every OTHER list is one value, which a later file replaces whole. (It
    /// used `secrets.names`, which stacks since review stop 12's Q11: a
    /// project's file adds caution, never replaces yours.)
    #[test]
    fn other_lists_are_still_replaced() {
        let s = stack(&[
            ("mine", "[tui]\nctx_colors = [\"red\"]\n"),
            ("proj", "[tui]\nctx_colors = [\"blue\"]\n"),
        ]);
        assert_eq!(s.config().0.tui.ctx_colors, ["blue"]);
    }
}
