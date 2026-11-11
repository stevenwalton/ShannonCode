//! User configuration — the customization substrate.
//!
//! The shape follows one rule, taken from the vision doc: **defaults live
//! in code; the user's file is a sparse diff over them, and nothing but you
//! writes it** — `:config edit` saves what you wrote, and nothing adds to
//! it. A user `config.toml` should contain only the keys it wants to
//! change; every absent key falls back to the coded default. That is what
//! `#[serde(default)]` on every struct buys — a partial document fills
//! only what it mentions.
//!
//! Unknown-key handling is deliberately asymmetric (a review decision):
//! - **Nested** sub-structs use `deny_unknown_fields`, so a typo like
//!   `[tui.prompt] heigth = 12` is surfaced as an error rather than
//!   silently ignored (the anti-papercut posture).
//! - **Top-level** unknown keys are *captured and warned about*, not
//!   rejected, so an older binary reading a config written for a newer
//!   version (or carrying a plugin's keys) degrades with a warning instead
//!   of refusing to start. Forward-compat beats strictness at the root;
//!   the vision rails against tools that hard-fail on an unknown key.
//!
//! `Config` lives in `shannon-core`, not the TUI, because it is domain
//! data any frontend needs (design.md §1.6 derives turn identity from
//! model/params). The TUI-only knobs live under `tui` as **plain data**
//! (no `ratatui` types) so core never grows a rendering dependency; the
//! TUI converts them to styles/lengths at its edge.
//!
//! This is a *seed*, intentionally small. Fields are added alongside the
//! code that reads them — not speculatively — so the file never lists a
//! knob that does nothing. `tui.prompt.height` is the first, consumed by
//! `layout::split_frame`.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub mod set;
mod stack;
pub use stack::{
    ConfigStack, Listed, MAX_CONFIG_BYTES, Origin, PROJECT_DIR, Refused, Typed, project_models_dir,
    project_path, read_file, same_file,
};

/// Default visible-row count for the prompt pane (excludes the border).
/// The M1 placeholder value, now sourced from config instead of a `const`
/// in `layout.rs`.
pub const DEFAULT_PROMPT_HEIGHT: u16 = 10;

/// Sane bounds for `tui.prompt.height`. A height of 0 leaves no editable
/// rows; an enormous one starves the output pane. `Config::clamp` brings
/// out-of-range values into `[MIN, MAX]` and warns.
pub const MIN_PROMPT_HEIGHT: u16 = 1;
pub const MAX_PROMPT_HEIGHT: u16 = 100;

/// Root configuration. Deserialized from `config.toml`; `Default` is the
/// authoritative source of every coded default (and the contract the
/// `DEFAULT_TOML` reference is tested against).
// No `Eq`: the `unknown` capture holds `toml::Value`, which isn't `Eq`
// (it can contain floats). `PartialEq` is all we need (tests, change
// detection); the rest of the tree is `Eq`-clean.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub tui: TuiConfig,
    /// Graph-surgery knobs (squash summarization, future merge policies). Domain
    /// data — the instruction shapes what a backend is asked, not how it renders —
    /// so it lives beside `tui`, not under it.
    pub merge: MergeConfig,
    /// Child-session guardrails (spawn depth / fan-out and the posture at a
    /// limit). Domain policy any frontend enforces at spawn, so it lives in
    /// core beside `merge`.
    pub agents: AgentsConfig,
    /// `:continue` (turn resumption) knobs. The Rust field carries a trailing
    /// underscore only because `continue` is a keyword; the TOML section is
    /// `[continue]`, matching the command.
    #[serde(rename = "continue")]
    pub continue_: ContinueConfig,
    /// Prompt caching on the way out to the provider. A wire/billing knob, not
    /// a rendering one, so it lives in core beside `agents`.
    pub cache: CacheConfig,
    /// Per-turn request knobs — today just the reply-length ceiling.
    pub turn: TurnSettings,
    /// How an approved shell command is run (timeout, retries, output cap,
    /// shell). Domain policy — any frontend running a tool call needs it — so it
    /// lives in core beside `agents`.
    pub exec: ExecSettings,
    /// Shannon's own file tools (`Read`, `Edit`, `Write`): what a change asks
    /// of the model's reading first. Domain policy, beside `exec`.
    pub files: FilesConfig,
    /// Credential scrubbing on the way out — to a provider, and to our own
    /// store. Domain policy, not rendering, so it lives in core beside `exec`.
    /// See `docs/DesignDocs/secrets.md`.
    pub secrets: crate::SecretsConfig,
    /// Your permission rules, over the shipped defaults, and the shipped rules
    /// you drop. Domain policy, beside `secrets`.
    pub permissions: PermissionsConfig,
    /// Claude Code as a provider: what the gate does with the tools its rules
    /// cannot read. Domain policy, beside `permissions`. See
    /// `docs/DesignDocs/claude-code-backend.md`.
    pub claude_code: ClaudeCodeConfig,
    /// Instruction files and memories — what the model is told beyond the
    /// conversation — and where it writes memories.
    pub memory: MemoryConfig,
    /// Where skills are found.
    pub skills: SkillsConfig,
    /// MCP servers — today, the ones Claude Code runs: which are not used.
    pub mcp: McpConfig,
    /// The session's substitution table, declared by the user: `[[alias]]`
    /// entries naming a local value that should travel as a short name.
    ///
    /// Top-level rather than under `[secrets]` on purpose — an alias for a repo
    /// path is not a credential setting. Sensitivity is a *property of an entry*
    /// (`secrets.md` §3a), not of the mechanism.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alias: Vec<AliasSpec>,
    /// User keymap overrides — a sparse diff over the built-in canonical-vim
    /// bindings: `context -> chord -> action-name`, all **plain strings** so
    /// core stays keymap-agnostic (the TUI parses, validates, and merges them).
    /// E.g. `[keymap.normal]` with `"<Enter>" = "submit"`, or `[keymap.motion]`
    /// with `"j" = "word-forward"`. Empty by default — the defaults live in
    /// code; an empty map serializes to nothing, so the
    /// `DEFAULT_TOML` reference needs no `[keymap]` section.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keymap: BTreeMap<String, BTreeMap<String, String>>,
    /// Unknown top-level keys, captured rather than rejected (see module
    /// docs). Surfaced by [`Config::load`] as warnings and otherwise
    /// inert, and **never serialized**: everything that shows or diffs the
    /// config walks its serialized form, so a captured section — a newer
    /// release's, a plugin's `[mcp.servers.x] token = …` — was printed by
    /// bare `:set` and `:set all`, listed by `:set all` while `:set` called
    /// it unknown, and reported "changed" by `:source` beside "ignored"
    /// (Reviewers A and B, 2026-09-29). `review` reads the field directly.
    #[serde(flatten, skip_serializing)]
    unknown: BTreeMap<String, toml::Value>,
}

/// `[permissions]`: what you have taught the gate, in the rule grammar
/// `:analyze` names rules in.
///
/// **On top of the shipped defaults, never instead of them by accident.** A
/// shipped rule goes only when `remove` names it, one rule at a time and
/// visibly (Steven, 2026-09-30: "name it to remove it"). Read by
/// `RuleSet::configured`, which reports an entry it cannot honour rather than
/// dropping it in silence.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PermissionsConfig {
    /// Rules, one per entry: `allow uv run pytest *`, `deny rm:-r`.
    pub rules: Vec<String>,
    /// Shipped rules to drop, each written as `:rules` lists it
    /// (`ask find:-delete`).
    pub remove: Vec<String>,
    /// **The scratch directory** — the agent's place for temporary files: an
    /// approved place at every level, and where Claude Code keeps its own.
    /// `None`: `/tmp/shannon-<uid>` (`scratch::default_path`). Used only once
    /// `scratch::prepare` vouches for it; otherwise launch says why, and
    /// nothing is approved there.
    pub scratch: Option<String>,
}

/// `[files]`: Shannon's own file tools (`file-tools.md`, S21-Q26). Read per
/// call (`App::file_context`), so a change applies to the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FilesConfig {
    /// **What an Edit, or a Write over a file, asks the model to have read
    /// of it first** (S21-Q20, Steven 11-02: "By default we'll allow the
    /// partial read but an option would require reading the full
    /// document").
    pub read_first: ReadFirst,
}

/// `[files] read_first` ([`FilesConfig`]): `"any"`, `"whole"` or `"off"`.
/// What the model has read is what the context it was sent still holds of
/// the file, as it is now (`files::Stamp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadFirst {
    /// Some of it — a partial read passes, as tokens are dear.
    #[default]
    Any,
    /// Every line of it.
    Whole,
    /// Nothing: `old_string` must still match.
    Off,
}

/// `[claude_code]`: how Shannon's gate treats the calls Claude Code runs, and
/// what Claude Code may do to its own context.
///
/// Read per call (`App::admit_provider_call`, `App::rules_for_call`) or per
/// request (`auto_compact`), so a change applies to the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClaudeCodeConfig {
    /// `WebFetch` and `WebSearch` (Steven, 10-03: "default ask is a good
    /// default", with a knob to allow them).
    pub web: WebPolicy,
    /// **A credential path, on a call Claude Code runs** — in place of
    /// `[secrets] path_policy` for those calls. Deny by default (Steven,
    /// 10-04): Claude Code's model sees its tools' output as it is, with no
    /// redaction between, so on this path the gate is the only layer.
    pub path_policy: crate::PathPolicy,
    /// **Whether Claude Code compacts its context on its own** near its
    /// window's end (Steven, 10-15: "are we forced to use autocompaction?").
    /// On by default — Claude Code's own behaviour; off, its window fills
    /// until a request is refused, and `:compact` is yours to run. Either
    /// way a compaction Claude Code makes is said (`compact_boundary`).
    pub auto_compact: bool,
    /// **Whether Claude Code may run work in the background** — agents with
    /// `run_in_background`, and Bash's. On by default, Claude Code's own
    /// behaviour (Steven, 10-22); but a background agent's calls never
    /// reach Claude Code's output, so the gate cannot show them and refuses
    /// every one. Off (`CLAUDE_CODE_DISABLE_BACKGROUND_TASKS`), that work
    /// runs in the foreground, where each call is shown — "let people close
    /// the gap if they want. We can experiment with it too."
    pub background_tasks: bool,
    /// **What `claude` is given of your environment** — and so every Bash
    /// command Claude Code runs (Steven, 10-31, S18-Q1: opt in). `inherit`,
    /// the default: all of it, Claude Code as you set it up, less what is
    /// never handed on (`ProviderEnv::withheld`). `minimal`: only what
    /// Shannon's own commands get (`exec::child_env`, with `[exec] env`)
    /// and Claude Code's own config directory — so a credential in your
    /// environment that Shannon does not hold (`AWS_SECRET_ACCESS_KEY`) is
    /// not there to print. Only your own config sets it.
    pub env: EnvPolicy,
}

impl Default for ClaudeCodeConfig {
    fn default() -> Self {
        Self {
            web: WebPolicy::default(),
            path_policy: crate::PathPolicy::Deny,
            auto_compact: true,
            background_tasks: true,
            env: EnvPolicy::default(),
        }
    }
}

/// What a provider's process is given of Shannon's environment —
/// `[claude_code] env`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvPolicy {
    /// All of it, less what is never handed on.
    #[default]
    Inherit,
    /// What Shannon's own commands get, and the provider's own config dir.
    Minimal,
}

/// Whether Claude Code's web tools ask or are allowed — `[claude_code] web`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebPolicy {
    /// Every fetch and search asks.
    #[default]
    Ask,
    /// Allowed unasked: the model may send whatever it has read to any URL.
    Allow,
}

/// `[memory]`: what the model is told beyond the conversation — instruction
/// files and memory indexes, read into a context block the episode records
/// (`knowledge`) — and where it may write memories.
/// `docs/DesignDocs/memory-and-skills.md`.
///
/// **One grammar for every location** (`knowledge::locate`): `~`, `/`,
/// `{config}` or `{start}` leading names that one place; anything else is
/// looked for from where Shannon started up to the project's root
/// ([`project_root`](MemoryConfig::project_root)).
/// `{project}` is filled in wherever it stands and places nothing. A place
/// may be a file or a directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryConfig {
    /// Whether the model writes memories (`:set memory=`), DECIDED by Steven
    /// (TODO (40)): `auto` — writes to [`write`](Self::write) run unasked;
    /// `ask` — they ask; `off` — the model is not told how.
    pub mode: MemoryMode,
    /// Instruction files, read whole into every session: `AGENTS.md` and
    /// `SHANNON.md` by default (Steven, 10-06, D1); a directory is every
    /// `*.md` in it. STACKED across config files.
    pub instructions: Vec<String>,
    /// Claude Code's own files — `CLAUDE.md` and `.claude/skills`.
    pub claude: ClaudeFiles,
    /// Where memories are READ: a directory (its `MEMORY.md` read, its other
    /// files listed) or a file (read whole). STACKED.
    pub paths: Vec<String>,
    /// Where the model WRITES memories: a directory, made if missing and
    /// vouched as the scratch directory is. Only your own config sets it — a
    /// file that is not yours setting it is ignored, and said
    /// (`config::stack`). Empty: nowhere.
    pub write: String,
    /// How `{project}` names this project (`knowledge::project_name`). Only
    /// your own config sets it: it decides where [`write`](Self::write) is.
    pub project: ProjectName,
    /// **What marks the project's root** — names looked for in where Shannon
    /// started and each directory above it; the nearest directory holding
    /// one is the root, where a relative location's walk stops. gutentags'
    /// `g:gutentags_project_root` with its default markers, the version
    /// controls' (Steven, 10-07). Empty, or none found: the walk is where
    /// Shannon started alone. Only your own config sets it: it decides what
    /// "the repository" is, which bounds what a relative location may read
    /// (`knowledge::locate::leaves_the_repository`). Claude Code's own
    /// naming of a project stays git's (`claude_import::project_key`).
    pub project_root: Vec<String>,
    /// What is NOT read — an instruction file or a memory, or a directory and
    /// all in it; locations as [`paths`](Self::paths) are written. Not told,
    /// not listed, and a read of it refused at the gate (Steven, 10-06:
    /// "block the file"); what the model already has stays. `:memories`'
    /// `x` sets it for the session. STACKED.
    pub disabled: Vec<String>,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            mode: MemoryMode::default(),
            instructions: vec!["AGENTS.md".into(), "SHANNON.md".into()],
            claude: ClaudeFiles::default(),
            paths: vec![
                "{config}/memory".into(),
                "{config}/projects/{project}/memory".into(),
            ],
            write: "{config}/projects/{project}/memory".into(),
            project: ProjectName::default(),
            project_root: [
                ".git",
                ".hg",
                ".svn",
                ".bzr",
                "_darcs",
                "_FOSSIL_",
                ".fslckout",
            ]
            .map(String::from)
            .to_vec(),
            disabled: Vec::new(),
        }
    }
}

/// **How `{project}` is named** — `[memory] project` (Steven, 10-06: "keep
/// the hash as the default but allow a Claude Code like option … extensible
/// later").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectName {
    /// The directory Shannon started in, by its name and the first 8 hex of
    /// the history store's hash of its path: `my-project-3f9a2c71`. Each
    /// directory its own project.
    #[default]
    Hash,
    /// As Claude Code names one: its repository's root, every character but
    /// an ASCII letter or digit a `-` (`-Users-me-my-project`) — so a
    /// subdirectory and every worktree of a repository are one project.
    ClaudeCode,
}

/// Whether the model writes memories — `[memory] mode`, `:set memory=`.
///
/// Read by hand so the decided spellings all land (TODO (40): "`memory=` =
/// off, `on` = auto").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryMode {
    /// The model is not told how to write memories, and no write there is
    /// approved. What `paths` names is still read — by default that
    /// includes the write location, so memories kept there are still told.
    Off,
    /// Writes to the write location run unasked.
    #[default]
    Auto,
    /// Writes to the write location ask, as any write outside does.
    Ask,
}

impl<'de> Deserialize<'de> for MemoryMode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let word = String::deserialize(d)?;
        match word.trim().to_ascii_lowercase().as_str() {
            "" | "off" => Ok(MemoryMode::Off),
            "auto" | "on" => Ok(MemoryMode::Auto),
            "ask" => Ok(MemoryMode::Ask),
            other => Err(serde::de::Error::custom(format!(
                "`{other}` is not a memory mode — off, auto, or ask"
            ))),
        }
    }
}

/// Claude Code's own files — `CLAUDE.md` and `.claude/skills` — `[memory]
/// claude` (Steven, 10-06: "`CLAUDE.md` is opt in, but default on when using
/// a claude code session").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaudeFiles {
    /// A Claude Code session reads them (`claude` does, and Shannon never
    /// sends them again); an API session does not.
    #[default]
    Auto,
    /// An API session reads them too.
    On,
    /// Neither: a Claude Code session runs with its `CLAUDE.md` loading off.
    Off,
}

/// `[skills]`: where skills are found — a directory (each `<name>/SKILL.md`
/// in it) or a file (one skill), in `[memory]`'s location grammar. STACKED.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkillsConfig {
    pub paths: Vec<String>,
    /// Skills NOT used — by name (`review`: every skill of that name), or a
    /// location and every skill in it (`~/.claude/skills`). Not listed, not
    /// loaded — by Shannon, or by Claude Code's `Skill` tool — and a read of
    /// one refused at the gate; what the model already has stays (Steven,
    /// 10-06). `:skills`' `x` sets it for the session. STACKED.
    pub disabled: Vec<String>,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            paths: vec!["{config}/skills".into(), ".shannon/skills".into()],
            disabled: Vec::new(),
        }
    }
}

/// `[mcp]`: MCP servers. Today they are Claude Code's — named by its `init`,
/// configured in its own settings (`claude mcp`) — so this says only which
/// are NOT used.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpConfig {
    /// Servers NOT used, by name: every call to one of its tools refused
    /// at the gate (Steven, 10-06: "This will be a bit more obvious with
    /// MCPs"). The server still starts, and its tools are still listed to
    /// the model; using one is what is refused. `:skills`' MCP list's `x`
    /// sets it for the session. STACKED.
    pub disabled: Vec<String>,
}

/// One declared substitution: a local value that travels as a short name.
///
/// Exactly one source is given. **Sensitivity is inferred from which**, and the
/// inference is principled rather than convenient: a value written in cleartext
/// in a config file has, by construction, already been decided not to be a
/// secret — nobody protecting a real credential can put it there. A value read
/// from the environment usually *is* one, since naming it here is the reason to
/// bother. Either way `sensitive` overrides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AliasSpec {
    /// The short name the wire sees: `repo`, `github-token`.
    pub name: String,
    /// A literal local value. Non-sensitive unless `sensitive = true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// An environment variable holding the value. Sensitive unless
    /// `sensitive = false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// Override the inference. `true` means the gate stops every command that
    /// spends it, and the handle gains a session nonce.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitive: Option<bool>,
}

impl AliasSpec {
    /// Does this entry gate? See the struct doc for why the source decides.
    pub fn is_sensitive(&self) -> bool {
        self.sensitive.unwrap_or(self.env.is_some())
    }
}

/// How an approved shell command is executed. Mirrors
/// [`exec::ExecConfig`](crate::exec::ExecConfig) in TOML-friendly units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecSettings {
    /// Seconds a single attempt may run before it is killed. A command that
    /// never returns must not hang the turn.
    #[serde(default = "default_exec_timeout")]
    pub timeout_secs: u64,
    /// Extra attempts after a failure. **`0` = off (the default)**, `-1` =
    /// unlimited.
    ///
    /// Off by default because a shell command is not assumed idempotent —
    /// re-running one that timed out can compound a partial effect. Turn it on
    /// only for commands you know are safe to repeat.
    #[serde(default)]
    pub max_retries: i32,
    /// Cap on captured output, in bytes.
    #[serde(default = "default_exec_max_output")]
    pub max_output_bytes: usize,
    /// How much of a tool call is written to the **log file**.
    ///
    /// Separate from the transcript (which always shows you what happened in
    /// the session) — this is the durable record on disk, and people differ:
    /// some want none of it, some want everything for an audit trail. Default
    /// records the *decision* but not the output, because output is the part
    /// most likely to contain secrets.
    #[serde(default)]
    pub audit: AuditLevel,
    /// The shell that interprets the command. **Keep this bash-compatible**: the
    /// permission analyzer models bash (process substitution `<(cmd)` is a
    /// bash-ism), so a shell with different semantics would mean the analysis
    /// you approved is not a description of what runs.
    #[serde(default = "default_exec_shell")]
    pub shell: String,
    /// Variables a command is given beyond the minimal set — `NAME =
    /// "value"`, `$NAME` in a value read from Shannon's own environment
    /// (`exec::configured_env`); `PATH` here is the one commands run with
    /// (yours, by default). Claude Code's process too. Only your own config
    /// sets it: it decides what a name your rules allow runs.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// **Variables no command is handed** — Shannon's own or Claude Code's,
    /// whatever else names them (Steven, 10-31: "a manual list where we can
    /// define variables Shannon gets or doesn't"; `env` above is what they
    /// get). A project's file may add to it, never take from it.
    #[serde(default)]
    pub withhold: Vec<String>,
}

impl Default for ExecSettings {
    fn default() -> Self {
        Self {
            timeout_secs: DEFAULT_EXEC_TIMEOUT_SECS,
            max_retries: 0,
            max_output_bytes: DEFAULT_EXEC_MAX_OUTPUT,
            audit: AuditLevel::default(),
            shell: DEFAULT_EXEC_SHELL.to_string(),
            env: std::collections::BTreeMap::new(),
            withhold: Vec::new(),
        }
    }
}

/// How much of a tool call reaches the log file.
///
/// The interesting record for a *security* question is what was authorized and
/// on what basis — which is why `Decisions` is the default, and why it captures
/// the verdict and the human's answer rather than the command's output. Output
/// is opt-in: it is the part most likely to carry credentials, and a log is a
/// durable artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditLevel {
    /// Write nothing about tool calls to the log.
    Off,
    /// The command, the engine's verdict, and how the human answered. **Default.**
    #[default]
    Decisions,
    /// Everything in `decisions`, plus the command's captured output.
    Full,
}

/// Coded defaults for `[exec]`.
pub const DEFAULT_EXEC_TIMEOUT_SECS: u64 = 120;
pub const DEFAULT_EXEC_MAX_OUTPUT: usize = 64 * 1024;
pub const DEFAULT_EXEC_SHELL: &str = "bash";

fn default_exec_timeout() -> u64 {
    DEFAULT_EXEC_TIMEOUT_SECS
}
fn default_exec_max_output() -> usize {
    DEFAULT_EXEC_MAX_OUTPUT
}
fn default_exec_shell() -> String {
    DEFAULT_EXEC_SHELL.to_string()
}

/// TUI-facing knobs. Plain data only — no `ratatui` types — so `shannon-core`
/// stays render-agnostic; the TUI maps these to concrete styles/lengths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TuiConfig {
    pub prompt: PromptConfig,
    /// Whether to capture the mouse: click focuses a pane, the wheel scrolls the
    /// pane under the cursor, drag selects. Default **on**; set `mouse = false`
    /// to leave the mouse entirely to the terminal (no capture). Even when on,
    /// **Shift-drag** falls through to the terminal's native selection.
    #[serde(default = "default_true")]
    pub mouse: bool,
    /// **Whether the arrow keys move** — in every pane, panel and float, as
    /// `h` `j` `k` `l` do. Default **on**; `arrows = false` turns them off for
    /// someone learning to keep their hands home (vim's `noremap <Up> <Nop>`),
    /// and an arrow then says why rather than nothing. Not on the `:`, `/`
    /// and `?` lines (one command line), where they recall history and move
    /// the cursor and no other key does,
    /// nor on a screen covering the frame (setup, the trust question), which
    /// has no row to say why on (Steven, 11-09: the tutor's "You can do the
    /// same with the arrow keys").
    #[serde(default = "default_true")]
    pub arrows: bool,
    /// **Whether a `y` also copies to the system clipboard** (OSC 52) — vim's
    /// `clipboard=unnamedplus`, for a yank. Default **on**: a `y` is how text
    /// leaves Shannon. Off, a yank fills only the register `p` pastes from,
    /// as vim's own default does (Steven, 11-09, for the tutor's note on the
    /// clipboard). A drag's copy is [`copy_on_select`](Self::copy_on_select)'s.
    #[serde(default = "default_true")]
    pub copy_on_yank: bool,
    /// Whether releasing a mouse **drag** copies what it selected to the system
    /// clipboard (OSC 52) — what a terminal's own selection does, and vim's
    /// `clipboard=autoselect`. The selection stays up, and `p`'s register is
    /// untouched. Default **off** (Steven, 10-06: "I'm not a big fan of that
    /// … disable by default"; coworkers complain of it in Claude Code and
    /// Copilot): a drag selects, and `y` copies, as in vim.
    #[serde(default)]
    pub copy_on_select: bool,
    /// What a copy to the system clipboard — a `y`, or a drag's release —
    /// carries for a secret it touches, whole or in part. Default **value**:
    /// the value is yours, and the model is never shown it — a command that
    /// prints the clipboard (`pbpaste`) asks first, and its output comes back
    /// with a held value as its handle. `handle` copies the handle instead;
    /// `skip` copies nothing and says why. The register `p` pastes from is
    /// never changed: it stays inside Shannon.
    #[serde(default)]
    pub copy_secrets: CopySecrets,
    /// The model new sessions start on — a short name (`haiku`, `sonnet`,
    /// `opus`, `fable`) or a full model id. Empty means the build's default
    /// (`backend::anthropic::DEFAULT_MODEL`, Sonnet).
    ///
    /// A *starting* value, not a lock: `:model` changes it per session, which
    /// is what makes two panes on two models possible. This exists so the
    /// answer to "why is everything Sonnet" is a config line rather than a
    /// recompile.
    #[serde(default)]
    pub model: String,
    /// Whether launching **resumes** the previous session or starts **fresh**.
    /// Default `fresh` — no surprise history on startup; the previous session
    /// stays on disk and is browsable in `:graph`.
    #[serde(default)]
    pub resume: ResumeMode,
    /// How `:graph` orders sibling sessions. Default `recency`.
    #[serde(default)]
    pub graph_sort: GraphSort,
    /// Which `:graph` visualization style to render. Default `git`.
    #[serde(default)]
    pub graph_style: GraphStyle,
    /// Glyph set for the `:graph` lane graph: `unicode` or `ascii`. Default
    /// `unicode`.
    #[serde(default)]
    pub graph_chars: GraphChars,
    /// Where `:graph` shows a named session's id: `after` its name (the
    /// default) or `hidden`. An unnamed session's and a turn's always show.
    #[serde(default)]
    pub graph_ids: GraphIds,
    /// Whether destructive deletes (`:delete`, `:delete all`, `:delete <hash>`,
    /// and `d` in `:graph`) ask for a `y`/`n` confirmation first. Default **on**;
    /// set `confirm_delete = false` to delete immediately.
    #[serde(default = "default_true")]
    pub confirm_delete: bool,
    /// Where `:split` puts the new pane — vim's `splitbelow`. Default **off**,
    /// which is vim's default and means the new pane appears **above** the one
    /// you split; `true` puts it below.
    #[serde(default)]
    pub splitbelow: bool,
    /// Where `:vsplit` puts the new pane — vim's `splitright`. Default **off**
    /// (vim's default): the new pane appears to the **left**; `true` puts it to
    /// the right.
    #[serde(default)]
    pub splitright: bool,
    /// Whether a search (`/ ? n N * #`) ignores case — vim's `ignorecase`.
    /// Default **off** (vim's default): `/mara` finds `mara`, not `Mara`. A
    /// `\c` or `\C` in a pattern decides for it, either way.
    #[serde(default)]
    pub ignorecase: bool,
    /// With `ignorecase` on, a pattern you type with a capital in it matches
    /// case — vim's `smartcase`: `/mara` finds `Mara`, `/Mara` only `Mara`.
    /// Not for `*` and `#`, whose word you did not type (vim's rule, which
    /// `n` after them keeps). Default **off** (vim's default).
    #[serde(default)]
    pub smartcase: bool,
    /// Rows of context kept visible above/below the cursor before a pane scrolls
    /// — vim's `scrolloff`. The soft ceiling and floor: the cursor pushes the
    /// view when it reaches them, and nothing moves in between. Honoured by
    /// every window — the prompt, the transcript, `:graph`, every page and the
    /// decision pane's report. An oversized value degrades to
    /// keep-centered (vim's `so=999` idiom), so no upper clamp is needed.
    #[serde(default = "default_scrolloff")]
    pub scrolloff: u16,
    /// Upper bound on the approval pane's height, in content rows. The pane
    /// sizes itself to **its content** — a short report gets a short box — and
    /// this only caps how much of the transcript a long one may borrow. Content
    /// drives; the cap protects the conversation behind it.
    #[serde(default = "default_approval_max_height")]
    pub approval_max_height: u16,
    /// **How long a call in the approval pane waits before a key answers
    /// it**, in milliseconds, from when it is shown — and again from each
    /// answer key pressed too soon, so a held or sticky key never answers.
    /// The pane held for a session's next call fills under your cursor when
    /// a command ends, and `a a` allowed the next call unread (Firefox's
    /// `security.dialog_enable_delay`; Steven, 10-19: "a few hundred ms…
    /// configurable… disable it (with 0)"). Default 300: past a double-tap,
    /// short of reading.
    #[serde(default = "default_approval_delay_ms")]
    pub approval_delay_ms: u64,
    /// Whether a shell command recorded in the transcript is **pretty-printed**
    /// (control flow on its own indented lines, as the approval gate shows it)
    /// rather than echoed as one long line. Default **on**: deciding on a
    /// laid-out command and then finding a wall of shell in the history is a
    /// needless discontinuity. Set `false` to keep commands verbatim on one line.
    #[serde(default = "default_true")]
    pub pretty_commands: bool,
    /// **Past how many columns a command is laid out across lines**: a chain
    /// at its `&&`/`||`, a pipeline at its `|`, a command before a run of its
    /// flags — packed greedily, a flag kept with its value. Default 72; `0`
    /// never breaks for length (control flow still gets its own lines). The
    /// pane, the transcript and `e` all lay a command out by it.
    #[serde(default = "default_command_width")]
    pub command_width: u16,
    /// **At most how many flags on a line of a command broken for length** —
    /// `1` puts each flag (with its value) on a line of its own. Default `0`:
    /// as many as fit in `command_width`. A command short enough for one line
    /// stays on one line whatever this says (Steven, 10-05: "If it is by
    /// number of flags, we should be able to set that").
    #[serde(default)]
    pub flags_per_line: u16,
    /// What `:set` says after it changes an option: `default` (the value you
    /// got), `quiet` (vim's silence) or `verbose` (the value and every layer
    /// that sets it, as `:verbose set x?` shows). Steven asked for "a silent,
    /// echo, and verbose option", and named them — `default` leaving room for
    /// what the everyday report grows into.
    #[serde(default)]
    pub set_report: SetReport,
    /// **What every colour looks like**: a theme by name — `terminal`, which
    /// names terminal colours so the terminal's own palette decides them, a
    /// file `themes/<name>.toml` beside this config, or one the TUI ships
    /// (`catppuccin-mocha`, `nord`, …). Applied by `:reload` and `:set`, to
    /// everything already on screen.
    #[serde(default = "default_theme")]
    pub theme: String,
    /// **Where else themes are found**: directories of terminal palettes —
    /// a ghostty or iTerm2 theme file, sixteen colours and a ground — each
    /// a theme by its file's name. `ghostty` is ghostty's own theme
    /// directories, wherever this machine keeps them (Steven, 10-14: "draw
    /// inspiration from vim and ghostty"). Only colours are read from them.
    #[serde(default = "default_theme_import")]
    pub theme_import: Vec<String>,
    /// **The colours the status line's `ctx` takes as the window fills** —
    /// `"percent:colour"` stops, each colour from its percent up: a
    /// terminal colour's name, its number or `#rrggbb` (Steven, 10-14:
    /// "Set as an array with `percentage:#hex`"). Two stops are thresholds;
    /// many make a stepped gradient. Below the first, the row's own colour.
    /// The TUI reads the colours (`theme::ctx_stops`) and says what it
    /// cannot.
    #[serde(default = "default_ctx_colors")]
    pub ctx_colors: Vec<String>,
    /// **The cursor's shape in each mode** — neovim's `'guicursor'`, as
    /// neovim writes it: `mode-list:argument-list` parts, a bar in Insert
    /// and a block in Normal by default, so a change of mode shows where
    /// you type as well as on the status row (Steven, 11-02). Empty leaves
    /// the terminal's own shape, as `:set guicursor=` does. The TUI reads it
    /// (`guicursor::Shapes::read`) and says what it cannot.
    #[serde(default = "default_guicursor")]
    pub guicursor: String,
    /// **What the status row says** in each kind of pane — see
    /// [`StatusConfig`].
    #[serde(default)]
    pub status: StatusConfig,
    /// **Which of a transcript's blocks start shut** — see [`FoldConfig`].
    #[serde(default)]
    pub fold: FoldConfig,
    /// **What applies to every alert** — see [`AlertsConfig`].
    #[serde(default)]
    pub alerts: AlertsConfig,
    /// **Each alert, by name** (`[tui.alert.<name>]`) — see [`AlertConfig`]:
    /// the shipped ones ([`SHIPPED_ALERTS`]) and yours. A table you write
    /// lays its keys over a shipped one of the same name.
    #[serde(default = "shipped_alerts", deserialize_with = "alerts_over_shipped")]
    pub alert: BTreeMap<String, AlertConfig>,
    /// **How a time is shown** (the inspector's): `locale` — your locale's
    /// date and time; `unix` — the seconds since 1970 it is saved as; any
    /// other text a `strftime` pattern (`%Y-%m-%d %H:%M`). What is saved is
    /// always the unix time (Steven, 10-14).
    #[serde(default = "default_time_format")]
    pub time_format: String,
    /// Per-role node glyphs for `:graph` — a sparse diff over the set picked by
    /// `graph_chars`, the same pattern as `keymap` (defaults live in code; the
    /// file names only what it changes). Roles: `session` (root/branch head),
    /// `child` (a spawned child session), `clone`, `merge`, `running` (the
    /// busy marker). Values are **single characters** (lane columns must stay
    /// aligned); the TUI validates and falls back with a warning. Plain
    /// strings so core stays render-agnostic — and the seed of the M4 theme
    /// config, which grows alongside it rather than replacing it.
    ///
    /// Serialized even when empty, so `:set tui.graph_glyphs.child=…` finds a
    /// section to put the role in (`set::room_for`).
    #[serde(default)]
    pub graph_glyphs: BTreeMap<String, String>,
}

/// Coded default for `tui.scrolloff`. Vim ships 0, but a small margin is the
/// well-loved vim-user setting and directly answers the "cursor pinned to the
/// window edge" papercut — the configurable knob keeps purists at 0 if they want.
pub const DEFAULT_SCROLLOFF: u16 = 5;

fn default_scrolloff() -> u16 {
    DEFAULT_SCROLLOFF
}

/// Coded default for `tui.approval_max_height`. Roomy enough for a headline, a
/// multi-stage command, a couple of findings and the action line without
/// scrolling, while still leaving the transcript the larger share of the frame.
pub const DEFAULT_APPROVAL_MAX_HEIGHT: u16 = 18;

fn default_approval_max_height() -> u16 {
    DEFAULT_APPROVAL_MAX_HEIGHT
}

/// Coded default for `tui.approval_delay_ms`: longer than a double-tap
/// (100–250 ms), too short to notice when you are reading.
pub const DEFAULT_APPROVAL_DELAY_MS: u64 = 300;

/// The longest `tui.approval_delay_ms` may be (`Config::clamp`): twice
/// Firefox's dialog delay, and short enough to wait out on purpose.
pub const MAX_APPROVAL_DELAY_MS: u64 = 2_000;

fn default_approval_delay_ms() -> u64 {
    DEFAULT_APPROVAL_DELAY_MS
}

/// Coded default for `tui.command_width`: a comfortable reading width, with
/// room for the pane's indent and a hung continuation inside 80 columns.
pub const DEFAULT_COMMAND_WIDTH: u16 = 72;

fn default_command_width() -> u16 {
    DEFAULT_COMMAND_WIDTH
}

/// Coded default for `tui.theme`: the theme the TUI ships, which needs no
/// file (shannon-tui's `Theme::DEFAULT` is this).
pub const DEFAULT_THEME: &str = "terminal";

fn default_theme() -> String {
    DEFAULT_THEME.to_string()
}

/// Coded default for `tui.theme_import`: ghostty's theme directories.
fn default_theme_import() -> Vec<String> {
    vec!["ghostty".to_string()]
}

/// Coded default for `tui.ctx_colors` (Steven, 10-14: "thresholds are 70%
/// and 90%"): caution, then danger, in the terminal's own colours.
fn default_ctx_colors() -> Vec<String> {
    vec!["70:yellow".to_string(), "90:red".to_string()]
}

fn default_time_format() -> String {
    "locale".to_string()
}

/// Neovim's default `'guicursor'` less its Terminal-mode part, a mode
/// Shannon has no use for (`options.txt`, neovim 0.12).
fn default_guicursor() -> String {
    "n-v-c-sm:block,i-ci-ve:ver25,r-cr-o:hor20".to_string()
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            prompt: PromptConfig::default(),
            mouse: true,
            arrows: true,
            copy_on_yank: true,
            copy_on_select: false,
            copy_secrets: CopySecrets::default(),
            model: String::new(),
            resume: ResumeMode::default(),
            graph_sort: GraphSort::default(),
            graph_style: GraphStyle::default(),
            graph_chars: GraphChars::default(),
            graph_ids: GraphIds::default(),
            confirm_delete: true,
            splitbelow: false,
            splitright: false,
            ignorecase: false,
            smartcase: false,
            scrolloff: DEFAULT_SCROLLOFF,
            approval_max_height: DEFAULT_APPROVAL_MAX_HEIGHT,
            approval_delay_ms: DEFAULT_APPROVAL_DELAY_MS,
            pretty_commands: true,
            command_width: DEFAULT_COMMAND_WIDTH,
            flags_per_line: 0,
            set_report: SetReport::default(),
            theme: default_theme(),
            theme_import: default_theme_import(),
            ctx_colors: default_ctx_colors(),
            guicursor: default_guicursor(),
            status: StatusConfig::default(),
            fold: FoldConfig::default(),
            alerts: AlertsConfig::default(),
            alert: shipped_alerts(),
            time_format: default_time_format(),
            graph_glyphs: BTreeMap::new(),
        }
    }
}

/// Which `:graph` visualization style to render. A seam, not a fixed look —
/// people have different visualization workflows (git-graph ships several too).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphStyle {
    /// Indented collapsible tree with box-drawing connectors.
    Tree,
    /// `git log --graph`-flavored: per-branch-colored commit dots (`●`),
    /// `(HEAD)` / branch decoration, and a compact one-line summary. Still
    /// collapsible. The default — this is the look the graph is built around.
    #[default]
    Git,
}

/// The glyph set the `:graph` lane graph draws with. `Unicode` uses box-drawing
/// with round corners; `Ascii` falls back to plain `| \ * -` for terminals or
/// fonts without good Unicode (the `git log --graph` look).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphChars {
    /// Box-drawing + round corners (`● │ ├ ╰`). The default.
    #[default]
    Unicode,
    /// ASCII-only (`* | + \`), for limited terminals.
    Ascii,
}

/// **Where `:graph` shows a named session's id** (Steven, 10-14: "show the
/// human readable name first … display the ID second or not"). Only a NAMED
/// session's: a session with no name, and a turn, have nothing else to be
/// called by, so their id stands where a name would.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphIds {
    /// After the name, dim: `auth-fix 1a2b3c4d`. The default.
    #[default]
    After,
    /// Not on the row — the inspector beside it still says it.
    Hidden,
}

/// What a copy to the system clipboard carries for a secret it touches — see
/// [`TuiConfig::copy_secrets`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CopySecrets {
    /// The value, as the screen shows it. The default.
    #[default]
    Value,
    /// The secret's handle, in place of every part of it the copy touched.
    Handle,
    /// Nothing: a copy touching a secret is not sent, and the notice says so.
    Skip,
}

/// How `:graph` orders sibling sessions (root episodes, and later branches).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphSort {
    /// Most-recently-active first — by last-turn time; a blank (just-started)
    /// session sorts newest. The default.
    #[default]
    Recency,
    /// By content hash (id bytes) — stable but arbitrary; the old behavior.
    Hash,
}

/// What a launch does with the previous session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResumeMode {
    /// Start a new, empty session each launch (the default). Previous sessions
    /// are kept on disk and reachable through `:graph`; resuming is deliberate.
    #[default]
    Fresh,
    /// Resume the last active session — the episode `HEAD` points at.
    Last,
}

/// What `:set` says after a change ([`TuiConfig::set_report`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SetReport {
    /// Nothing, as vim says nothing.
    Quiet,
    /// The value you got, `key=value` — adjusted, if the config had to.
    #[default]
    Default,
    /// The value and every layer that sets it, top first, down to the
    /// default — what `:verbose set x?` shows.
    Verbose,
}

/// `#[serde(default)]` helper — a missing `bool` key defaults to `true` (used by
/// `tui.mouse`, where absent means "on", not `bool`'s `false`).
fn default_true() -> bool {
    true
}

/// The instruction sent to the model when `S` asks it to distill turns into a
/// squash summary. One place, so the coded default and a user's override flow
/// through the same seam. First person because the summary lands *as the
/// assistant's message* in the merged history — the model is summarizing its
/// own side of the work.
pub const DEFAULT_SQUASH_INSTRUCTION: &str = "Summarize this conversation into one \
concise report of what was done, decided, and learned, in the assistant's first \
person. Preserve any paths, commands, code, or identifiers needed to continue the \
work. Output only the summary.";

/// Graph-surgery configuration (design.md §1.2). Small on purpose: fields join it
/// alongside the code that reads them (a summary-model override, per-strategy
/// templates) — never speculatively.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MergeConfig {
    /// What the model is asked when generating a squash summary (`S` in `:graph`).
    /// The generated reply seeds `$EDITOR` for review — this instruction is the
    /// user's lever on what that draft emphasizes.
    #[serde(default = "default_squash_instruction")]
    pub squash_instruction: String,
    /// **How many of a session's last turns `:compact` keeps as they were**
    /// — the rest become one summary (asked with `squash_instruction`).
    /// None by default: the whole history is summarised, as Claude Code's
    /// `/compact` does (Steven, 10-16: "I want the default `:compact` to act
    /// like Claude Code's default `/compact`"). It kept the last exchange.
    #[serde(default = "default_compact_keep")]
    pub compact_keep: usize,
    /// **Whether `:compact`'s summary opens in `$EDITOR` before it replaces
    /// anything** (Steven, 10-15: "a setting that lets you auto accept the
    /// compaction result"). `:compact!` does the other for once. Off by
    /// default since 10-16, as `/compact` asks nothing; it was on.
    #[serde(default = "default_compact_review")]
    pub compact_review: bool,
    /// **Whether `S`'s summary (`:distill`) opens in `$EDITOR` before it
    /// composes** — as `compact_review` is `:compact`'s (Steven, 10-21:
    /// "Distilling is like compaction so they should act similarly").
    /// `:distill!` does the other for once. Off by default, as compaction
    /// is; it was always on, so a summary arriving took the terminal into
    /// `$EDITOR` whatever you were doing (full review 10-21, A 8).
    #[serde(default = "default_distill_review")]
    pub distill_review: bool,
}

impl Default for MergeConfig {
    fn default() -> Self {
        Self {
            squash_instruction: DEFAULT_SQUASH_INSTRUCTION.to_string(),
            compact_keep: DEFAULT_COMPACT_KEEP,
            compact_review: default_compact_review(),
            distill_review: default_distill_review(),
        }
    }
}

fn default_compact_review() -> bool {
    false
}

fn default_distill_review() -> bool {
    false
}

/// Coded default for `merge.compact_keep`: nothing kept verbatim — the whole
/// history becomes the summary, as `/compact` makes it.
pub const DEFAULT_COMPACT_KEEP: usize = 0;

fn default_compact_keep() -> usize {
    DEFAULT_COMPACT_KEEP
}

fn default_squash_instruction() -> String {
    DEFAULT_SQUASH_INSTRUCTION.to_string()
}

/// The instruction `:continue`'s default (`instruct`) strategy sends, with
/// `{tail}` replaced by the end of the truncated reply. Config, not code: how
/// a user wants resumption phrased is theirs to decide. First person to the
/// model ("your previous response") because it rides in a user turn.
pub const DEFAULT_CONTINUE_INSTRUCTION: &str = "Your previous response was \
interrupted and ended with:\n\n{tail}\n\nContinue exactly from where you left \
off -- do not repeat or summarize what you already wrote; pick up mid-sentence \
if that is where it stopped.";

/// How `:continue` resumes a truncated/interrupted turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContinueStrategy {
    /// Send a normal follow-up user turn quoting the tail of the partial
    /// (the instruction template below). Works on **every** model — the
    /// default. The resumed text arrives as a new turn; squash/edit can
    /// merge the pair afterwards if one seamless turn is wanted.
    #[default]
    Instruct,
    /// Re-send the transcript ending with the partial assistant message and
    /// let the model continue it in place — an exact splice, composed back
    /// into ONE turn. Model-gated: the 4.6+/5-family models reject trailing
    /// assistant messages (HTTP 400); Haiku-class models accept them. The
    /// rejection surfaces on the notice line like any backend error.
    Prefill,
}

/// `:continue` configuration — the turn-resumption strategy and the
/// instruction template the `instruct` strategy sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContinueConfig {
    /// `instruct` (universal, default) or `prefill` (exact splice, model-gated).
    #[serde(default)]
    pub strategy: ContinueStrategy,
    /// The follow-up the `instruct` strategy sends; `{tail}` is replaced with
    /// the end of the truncated reply.
    #[serde(default = "default_continue_instruction")]
    pub instruction: String,
}

impl Default for ContinueConfig {
    fn default() -> Self {
        Self {
            strategy: ContinueStrategy::Instruct,
            instruction: DEFAULT_CONTINUE_INSTRUCTION.to_string(),
        }
    }
}

fn default_continue_instruction() -> String {
    DEFAULT_CONTINUE_INSTRUCTION.to_string()
}

/// Coded defaults for the child-session guardrails. Small numbers on purpose:
/// the limits exist to catch *runaway recursion and fan-out* (children spawning
/// children), and with `on_limit = "confirm"` they are a speed bump, not a
/// wall — hitting one asks y/N with the reason instead of refusing.
pub const DEFAULT_MAX_DEPTH: usize = 3;
pub const DEFAULT_MAX_FANOUT: usize = 4;

/// What happens when a child-session spawn would cross a `[agents]` limit.
/// A config enum, not a hardcoded policy — "hard block is a setting; good for
/// different situations" (the user's ruling). Plain data so any frontend can
/// enforce it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnLimit {
    /// Ask y/N with the reason before spawning (the default).
    #[default]
    Confirm,
    /// Refuse the spawn (with the reason surfaced).
    Block,
    /// Spawn anyway; just say the limit was crossed.
    Advisory,
}

/// Child-session (sub-agent) guardrails — bounds on the **ancestry tree**
/// (design.md §1.2): how deep spawn chains may recurse and how wide one
/// session may fan out. Only `ToolCall` (spawn) edges count against these;
/// branches/clones/merges are user surgery, not recursion — a human three
/// manual branches deep still spawns freely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentsConfig {
    /// Maximum spawn-chain depth: a session `max_depth` spawns deep may not
    /// start another child (the foreground session counts as depth 0 until
    /// it was itself spawned). `0` gates every spawn — a "always ask me"
    /// posture under `on_limit = "confirm"`.
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
    /// Maximum child sessions one session may spawn (width of the ancestry
    /// tree at one node, counting `ToolCall` edges only).
    #[serde(default = "default_max_fanout")]
    pub max_fanout: usize,
    /// The posture at a limit: `confirm` (y/N with the reason), `block`
    /// (refuse, reason surfaced), or `advisory` (spawn, note the crossing).
    #[serde(default)]
    pub on_limit: OnLimit,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_fanout: DEFAULT_MAX_FANOUT,
            on_limit: OnLimit::Confirm,
        }
    }
}

fn default_max_depth() -> usize {
    DEFAULT_MAX_DEPTH
}

fn default_max_fanout() -> usize {
    DEFAULT_MAX_FANOUT
}

/// Per-turn knobs on what is asked of the model.
///
/// Its own section rather than a field under `[cache]` or `[tui]`, because it
/// is neither the wire's business nor the renderer's: it is what this turn asks
/// for. Sampling parameters and thinking budgets land here when they land.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TurnSettings {
    /// Ceiling on a single reply, in tokens. `0` (the default) means **the
    /// model's own maximum** — Shannon imposes nothing the provider would not.
    ///
    /// The predecessor of this knob was a `const` of 4,096 with no way to
    /// change it, justified as stopping a small test-key budget being drained
    /// by a runaway turn. It truncated all seven turns of a live demo and four
    /// of them rendered nothing at all. Someone who genuinely wants that bound
    /// should have it — visibly, and by choice — which is what this is.
    ///
    /// `0` rather than `Option<u32>` so the TOML reads as a number in both
    /// cases; a cap of zero would mean "reply with nothing", which is not a
    /// thing anyone wants and makes the sentinel unambiguous.
    #[serde(default)]
    pub max_output_tokens: u32,
    /// The context budget you want to work inside, in tokens. `0` (the default)
    /// means **the model's own window**.
    ///
    /// Two real reasons to set it lower, both Steven's: a long context costs
    /// more on every subsequent turn, so a bound is a spend control; and long
    /// contexts get less reliable well before they get full, so a bound is a
    /// quality control. The model's window is a hard limit, not a
    /// recommendation, and 1M of it is a lot of rope.
    ///
    /// It moves the `ctx` gauge and the warning that comes with it. It cannot
    /// make a model accept more than it accepts — a value above the model's own
    /// window just makes the gauge optimistic, which is why the gauge names
    /// which bound it is measuring against.
    #[serde(default)]
    pub max_context_tokens: u32,
    /// **The default effort** — how much work the model is asked to put into
    /// a reply (`output_config.effort` on the API, `--effort` to `claude`) —
    /// for a session that has not chosen its own: a session's effort is its
    /// own, as its model is (`:effort`, kept in its header), and outranks
    /// this. `None` (`"default"` in the file) sends none: through the API the
    /// model's own default, through Claude Code the user's own Claude Code
    /// setting. Which levels a model takes is its row in `backend::models`;
    /// the app sends none where the model does not take it, and says so.
    /// `CLAUDE_CODE_EFFORT_LEVEL` sets it at launch, as a `:set` would
    /// (`ConfigStack::start_effort`).
    #[serde(default, with = "effort_setting")]
    pub effort: Option<crate::Effort>,
    /// **What thinking a model that thinks adaptively shows** (4.6 and
    /// later: `thinking.display`). `summarized` (the default — Steven, 10-27,
    /// NQ10: you asked to see thinking) is a summary of its reasoning and its
    /// notes between tool calls, mixed; `updates`, the notes alone, the
    /// reasoning never seen (a beta); `omitted`, empty blocks — the API's own
    /// default on 5.x, where a long tool-using turn goes quiet. Thinking
    /// happens and is billed the same under all three.
    #[serde(default)]
    pub thinking: ThinkingDisplay,
    /// **Thinking for a model that thinks only on a budget** (4.5 and
    /// earlier: `{type: "enabled", budget_tokens}`), in tokens. `0` (the
    /// default) asks for none — what those models do unasked, so nothing is
    /// billed you did not ask for. The API takes 1024 at least, and less than
    /// the reply's ceiling.
    #[serde(default)]
    pub thinking_budget: u32,
}

/// `[turn] thinking`: what a model that thinks adaptively shows of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingDisplay {
    /// Reasoning summaries and progress notes, mixed.
    #[default]
    Summarized,
    /// The progress notes alone (beta `thinking-display-updates-2026-08-18`).
    Updates,
    /// Empty blocks.
    Omitted,
}

/// `turn.effort` as the file spells it: a level, or `"default"` for none.
/// One enum for both the wire and the file — a second, config-only copy of
/// the levels would be a second list to drift.
mod effort_setting {
    use crate::Effort;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(effort: &Option<Effort>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(effort.map_or("default", Effort::as_str))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Effort>, D::Error> {
        Effort::parse_setting(&String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

impl TurnSettings {
    /// The ceiling to send, or `None` to use the model's own.
    pub fn ceiling(self) -> Option<u32> {
        (self.max_output_tokens > 0).then_some(self.max_output_tokens)
    }

    /// The context budget to measure against: the user's if they set one, else
    /// the model's own window.
    pub fn context_budget(self, model: &str) -> u32 {
        if self.max_context_tokens > 0 {
            self.max_context_tokens
        } else {
            crate::backend::models::context_window(model)
        }
    }

    /// Is the budget the user's choice rather than the model's limit? The
    /// gauge says which, because "83% full" means a different thing when the
    /// ceiling is one you picked.
    pub fn budget_is_yours(self) -> bool {
        self.max_context_tokens > 0
    }
}

/// How long a cache entry lives. The API's `cache_control.ttl`.
///
/// Not a duration: the API takes exactly these two, and a free-form
/// `Duration` would let a user write `10m` and get a 400 from the provider
/// instead of an answer from us. An enum makes the unrepresentable
/// unrepresentable, which is the whole reason to type a config field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CacheTtl {
    /// Five minutes — the default, and the cheaper write (1.25× base input).
    /// The clock restarts on every read, so an active conversation keeps its
    /// prefix alive indefinitely; the five minutes bound how long you can walk
    /// away and come back warm.
    #[default]
    #[serde(rename = "5m")]
    FiveMinutes,
    /// One hour — writes cost 2× base input instead of 1.25×. Worth it only
    /// when you branch off a prefix long after writing it, which is exactly the
    /// exploration-loop shape (`TODO.md`): fork six children off one turn an
    /// hour apart and the extra write is repaid by the first re-read.
    #[serde(rename = "1h")]
    OneHour,
}

impl CacheTtl {
    /// The wire spelling, or `None` for the default five minutes (which the API
    /// takes as the absence of `ttl`, not as a value).
    pub fn wire(self) -> Option<&'static str> {
        match self {
            Self::FiveMinutes => None,
            Self::OneHour => Some("1h"),
        }
    }
}

/// Prompt caching: whether a request asks the provider to cache its prompt
/// prefix, and for how long.
///
/// **On by default**, because a prefix that is written once and read once
/// already pays for itself (1.25× + 0.1× against 2× uncached), and because a
/// prompt below the model's minimum cacheable length is simply processed
/// uncached — no error, no surcharge. The knob exists for someone who wants
/// their spend to have exactly one shape.
///
/// What it does NOT control is where the breakpoint goes: the request carries a
/// top-level `cache_control`, which the API applies to the last cacheable block
/// and moves forward on its own as the conversation grows. That is the rolling
/// trail `TODO.md` describes — an entry left at every turn boundary, so a branch
/// taken from any turn still within the TTL re-reads the shared prefix instead
/// of re-paying for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CacheConfig {
    /// Ask the provider to cache the prompt prefix.
    pub enabled: bool,
    /// How long an entry lives once written.
    pub ttl: CacheTtl,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ttl: CacheTtl::FiveMinutes,
        }
    }
}

/// Which mode the prompt pane starts in. `Normal` by default — the
/// project's vim lineage is modal-first — but a user who'd rather land
/// ready to type sets `start_mode = "insert"`. Plain data (no keymap
/// types) so it stays in `shannon-core`; the TUI maps it to its `Mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartMode {
    #[default]
    Normal,
    Insert,
}

/// Prompt-pane configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PromptConfig {
    /// Visible editable rows (excludes the border). Field-level default so
    /// that `[tui.prompt]` present-but-without-`height` still yields the
    /// real default, not `u16`'s `0`.
    #[serde(default = "default_prompt_height")]
    pub height: u16,
    /// Mode the prompt starts in (`normal` | `insert`).
    #[serde(default)]
    pub start_mode: StartMode,
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            height: DEFAULT_PROMPT_HEIGHT,
            start_mode: StartMode::Normal,
        }
    }
}

fn default_prompt_height() -> u16 {
    DEFAULT_PROMPT_HEIGHT
}

/// **Which of a transcript's blocks have a fold, and start shut, by kind**
/// — `[tui.fold]` (`navigation.md` §6, NQ3; Steven, 10-27: "Tools and
/// thoughts are default folded. But they can be unfolded depending on
/// settings"). A fold you open or shut is your window's, for this run
/// (S21-Q2: "Only folds that stay are due to settings"); every other block
/// follows this. Read at every draw, so a `:set` shows on the next frame.
/// Your message and a reply have no fold until you make one (Steven,
/// 11-03: "Default should be that we manually fold that text, but not that
/// there is a fold there already. In the fold settings a user can set
/// that").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FoldConfig {
    /// What the model thought.
    pub thinking: FoldStart,
    /// A tool call's record.
    pub tool: FoldStart,
    /// The model's reply.
    pub reply: FoldStart,
    /// Your message.
    pub message: FoldStart,
}

impl FoldConfig {
    /// As shipped: thinking and tool records shut, whatever their length;
    /// a reply and your message folded only by hand.
    pub const SHIPPED: FoldConfig = FoldConfig {
        thinking: FoldStart::Fixed(Folded::Shut),
        tool: FoldStart::Fixed(Folded::Shut),
        reply: FoldStart::Fixed(Folded::Manual),
        message: FoldStart::Fixed(Folded::Manual),
    };
    /// Nothing starts shut.
    pub const OPEN: FoldConfig = FoldConfig {
        thinking: FoldStart::Fixed(Folded::Open),
        tool: FoldStart::Fixed(Folded::Open),
        reply: FoldStart::Fixed(Folded::Manual),
        message: FoldStart::Fixed(Folded::Manual),
    };
}

impl Default for FoldConfig {
    fn default() -> Self {
        FoldConfig::SHIPPED
    }
}

/// **How a kind of block starts** (`[tui.fold]`): `"shut"`, `"open"`,
/// `"manual"`, or `{ shut_over = N }` — shut when it holds more than N
/// lines, the count its shut row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    untagged,
    expecting = "\"shut\", \"open\", \"manual\", or { shut_over = N }"
)]
pub enum FoldStart {
    /// `"shut"`, `"open"` or `"manual"`, whatever its length.
    Fixed(Folded),
    /// `{ shut_over = N }`.
    Over(ShutOver),
}

impl FoldStart {
    /// Does a block of `lines` lines start shut?
    pub fn shuts(self, lines: usize) -> bool {
        match self {
            FoldStart::Fixed(folded) => folded == Folded::Shut,
            FoldStart::Over(over) => lines > over.shut_over,
        }
    }

    /// **Does a block of this kind have a fold before one is made by
    /// hand** — what `zM` and `zR` shut and open, as vim's shut every fold
    /// there is and leave lines with none drawn. `"manual"` has none.
    pub fn has_a_fold(self) -> bool {
        self != FoldStart::Fixed(Folded::Manual)
    }
}

/// `"shut"`, `"open"` or `"manual"` ([`FoldStart::Fixed`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Folded {
    /// Drawn whole.
    Open,
    /// Drawn as one row: its first line, and how many it holds.
    Shut,
    /// **No fold until you make one** (`zc`): drawn whole, and `zM` leaves
    /// it so — vim's manual folds, none until `zf`.
    Manual,
}

/// `{ shut_over = N }` ([`FoldStart::Over`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShutOver {
    /// The most lines a block holds and still starts open.
    pub shut_over: usize,
}

/// **What the status row says, per kind of pane** (Steven, 10-14: "make
/// this configurable … there should be macros associated with these
/// things. I think starship has a helpful syntax"). Each is a format the
/// TUI reads (`status_line::parse`) and says what it cannot: `$model` and
/// the rest are values, `{ … }` a group shown when a value in it is, and
/// the rightmost group goes first when the row is too narrow. Plain
/// strings, so core stays render-agnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StatusConfig {
    /// The prompt's row.
    pub prompt: String,
    /// The transcript's row.
    pub output: String,
    /// The decision pane's row.
    pub decision: String,
    /// `:graph`'s row.
    pub graph: String,
    /// Every read-only page's row — `:help`, `:rules`, `:ls`, the inspector
    /// and the rest; `$pane` and `$help` say which.
    pub page: String,
    /// How the rows write a token count: `10.4M` or `10415096`.
    pub tokens: TokenCounts,
}

/// **How the status row writes a token count** — `[tui.status] tokens`
/// (Steven, 10-15: "make these numbers human readable and do that by
/// default"). Read at every draw, so a `:set` shows on the next frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenCounts {
    /// `10.4M`, `40k`, `999`: one decimal, to the nearest, a trailing `.0`
    /// dropped. A session's `↑` reached eight digits in a day, which is a
    /// number to count rather than read.
    #[default]
    Short,
    /// Every digit, as the backend reported it.
    Exact,
}

/// Coded defaults for `[tui.status]` (Steven, 10-14, the first three as he
/// wrote them). `$alerts` is empty unless a decision, a queued prompt or a
/// command is waiting on you, and `$activity` unless something is running;
/// both sit left of the session's numbers, so a narrow row keeps them.
/// `${alert.hidden}` — a decision no pane shows — sits at the right edge
/// (Steven, 10-22: "by default it should go in the bottom on the right
/// side"), past a `$fill`, and so is not said again in `$alerts`. Placed as
/// the alert, not its value `$hidden`, so a narrow row keeps it too.
pub const DEFAULT_STATUS_PROMPT: &str = "[$mode] $pane{ · $alerts}{ · $activity}{ · $model}\
                                         { · ctx $ctx}{ · tok ↑$tokens_in{ ($cache)} ↓$tokens_out}\
                                         { $fill${alert.hidden}}";
/// See [`DEFAULT_STATUS_PROMPT`].
pub const DEFAULT_STATUS_OUTPUT: &str = "[$mode] $pane{ · $alerts}{ · $activity}\
                                         { · $model}{ · ctx $ctx}\
                                         { · tok ↑$tokens_in{ ($cache)} ↓$tokens_out}\
                                         { $fill${alert.hidden}}";
/// See [`DEFAULT_STATUS_PROMPT`].
pub const DEFAULT_STATUS_DECISION: &str =
    "[$mode] $pane{ · $alerts}{ · $activity}{ · help `:help $help`}{ $fill${alert.hidden}}";
/// See [`DEFAULT_STATUS_PROMPT`].
pub const DEFAULT_STATUS_GRAPH: &str = "[$mode] $pane · $position{ · $alerts}{ · $activity}\
                                        { · help `:help $help`}{ $fill${alert.hidden}}";
/// See [`DEFAULT_STATUS_PROMPT`].
pub const DEFAULT_STATUS_PAGE: &str = "[$mode] $pane · $position{ · $alerts}\
                                       { · help `:help $help`}{ $fill${alert.hidden}}";

impl Default for StatusConfig {
    fn default() -> Self {
        Self {
            prompt: DEFAULT_STATUS_PROMPT.to_string(),
            output: DEFAULT_STATUS_OUTPUT.to_string(),
            decision: DEFAULT_STATUS_DECISION.to_string(),
            graph: DEFAULT_STATUS_GRAPH.to_string(),
            page: DEFAULT_STATUS_PAGE.to_string(),
            tokens: TokenCounts::default(),
        }
    }
}

/// **`[tui.alerts]`: what applies to every alert** (`decision-panes-and-
/// alerts.md` §5.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AlertsConfig {
    /// Alerts not drawn, by name — a shipped one, or one of yours, as
    /// `[skills] disabled` removes a shipped skill. STACKED across files.
    pub disabled: Vec<String>,
    /// **What an alert's `mark` draws** beside a session in `:ls` (Steven,
    /// 10-22: "a bell icon when listing in `:ls`. We should be able to
    /// change the character if a user wants").
    pub glyph: String,
    /// **Whether any alert rings the terminal's bell** — every alert's
    /// `bell`, switched off at once (Steven, 10-23: "certainly some users
    /// will want to turn these off").
    pub bell: bool,
    /// **Whether any alert sends a desktop notification** — every alert's
    /// `notify`, switched off at once.
    pub notify: bool,
    /// **How a notification is written to the terminal**.
    pub notify_via: NotifyVia,
}

/// Coded default for `tui.alerts.glyph`.
pub const DEFAULT_ALERT_GLYPH: &str = "🔔";

impl Default for AlertsConfig {
    fn default() -> Self {
        Self {
            disabled: Vec::new(),
            glyph: DEFAULT_ALERT_GLYPH.to_string(),
            bell: true,
            notify: true,
            notify_via: NotifyVia::default(),
        }
    }
}

/// **How a desktop notification reaches the terminal** — `[tui.alerts]
/// notify_via`. Chosen by you, not guessed from `$TMUX` (Steven, 10-23: "I'm
/// not sure the `$TMUX` variable is reliable enough so I'd rather a more
/// manual approach").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyVia {
    /// `OSC 9`, which ghostty, iTerm2 and WezTerm show.
    #[default]
    Osc9,
    /// `OSC 9` wrapped for tmux to pass through, which it does only with
    /// `set -g allow-passthrough on`.
    Tmux,
}

/// **An alert's `notify`**: `true` for Shannon's own words, `false` for none,
/// or a message of yours (`"$session needs you"`), whose values the TUI reads
/// and says what it cannot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Notify {
    On(bool),
    Says(String),
}

impl<'de> Deserialize<'de> for Notify {
    /// By hand, for the sentence: serde's for an untagged enum is "data did
    /// not match any variant", which says nothing of what to write.
    fn deserialize<D: serde::Deserializer<'de>>(read: D) -> Result<Notify, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Notify;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("true, false, or a message such as \"$session needs you\"")
            }
            fn visit_bool<E: serde::de::Error>(self, on: bool) -> Result<Notify, E> {
                Ok(Notify::On(on))
            }
            /// **`"true"` and `"false"` are the switch**, never a
            /// notification of the word: `:set` types a value by what the
            /// key holds, and the shipped `notify` holds words, so `:set
            /// tui.alert.unshown_session.notify=false` arrived as the
            /// string — and every notification after it said "false"
            /// (review stop 9, A F3).
            fn visit_str<E: serde::de::Error>(self, says: &str) -> Result<Notify, E> {
                Ok(match says {
                    "true" => Notify::On(true),
                    "false" => Notify::On(false),
                    _ => Notify::Says(says.to_string()),
                })
            }
        }
        read.deserialize_any(Visitor)
    }
}

/// **`[tui.alert.<name>]`: one alert** — something waiting on you, by a
/// rule you write (Steven, 10-22: triggers, with outcomes). The TUI reads
/// both keys (`status_line::Alerts`) and says what it cannot.
///
/// Each key is optional, so a table you write over a shipped alert of the
/// same name changes only what it names (`alerts_over_shipped`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AlertConfig {
    /// **When it holds**: a fact (`queued`), or a fact compared with a
    /// whole number (`ctx_percent >= 80`). None: whenever `say` has
    /// something to say, as each shipped alert does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    /// **What the status row says of it**, where `$alerts` or
    /// `${alert.<name>}` is written: a status format, as `[tui.status]`'s
    /// are. None says nothing on the row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub say: Option<String>,
    /// **The border of each window showing a session it holds for**: a
    /// colour or a theme role (`ui.caution`), and a heavier line. Only an
    /// alert whose `when` names a session's fact has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border: Option<String>,
    /// **Whether `:ls` marks each session it holds for**, with
    /// `[tui.alerts] glyph`. Only an alert whose `when` names a session's
    /// fact marks one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mark: Option<bool>,
    /// **Whether the terminal's bell rings when it begins to hold** — once
    /// per session it holds for, or once for the app's fact. Needs a `when`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bell: Option<bool>,
    /// **Whether a desktop notification says so when it begins to hold**,
    /// and in what words ([`Notify`]). Needs a `when`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify: Option<Notify>,
}

impl AlertConfig {
    /// **This, with what it leaves unsaid taken from `under`** — key by key.
    /// Every key named, so a key added to the struct and not here does not
    /// compile: a file's new key silently dropped is the failure it rules out.
    fn over(self, under: AlertConfig) -> AlertConfig {
        let AlertConfig {
            when,
            say,
            border,
            mark,
            bell,
            notify,
        } = self;
        AlertConfig {
            when: when.or(under.when),
            say: say.or(under.say),
            border: border.or(under.border),
            mark: mark.or(under.mark),
            bell: bell.or(under.bell),
            notify: notify.or(under.notify),
        }
    }
}

/// **A shipped alert**, as [`SHIPPED_ALERTS`] writes one: each key it sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShippedAlert {
    pub name: &'static str,
    pub when: Option<&'static str>,
    pub say: Option<&'static str>,
    pub mark: bool,
    pub bell: bool,
    /// Its notification's words, if it sends one.
    pub notify: Option<&'static str>,
}

impl ShippedAlert {
    /// One said by its value of the same name, whenever that has something
    /// to say.
    const fn says(name: &'static str, say: &'static str) -> ShippedAlert {
        ShippedAlert {
            name,
            when: None,
            say: Some(say),
            mark: false,
            bell: false,
            notify: None,
        }
    }

    /// As `tui.alert` holds it.
    fn config(self) -> AlertConfig {
        AlertConfig {
            when: self.when.map(str::to_string),
            say: self.say.map(str::to_string),
            border: None,
            mark: self.mark.then_some(true),
            bell: self.bell.then_some(true),
            notify: self.notify.map(|says| Notify::Says(says.to_string())),
        }
    }
}

/// **The shipped alerts**, in the order `$alerts` says them. The first five
/// are what waits on you, each said by its value and holding whenever that
/// has something to say: the first four were `$alerts` before alerts were
/// rules (B1.1), `hidden` joined them in A1.5. The last two are news you
/// have not seen (Steven, 10-23: "let's ship the `unshown` alert … both"):
/// `unshown` says on the row the first session that has it and how many,
/// `unshown_session` marks each in `:ls`, rings the bell and sends a
/// notification ("I agree with shipping the bell and notify on by
/// default"). Two, because the row asks a session's facts of the one it is
/// about — your prompt's — while the news is of the others, and a mark
/// needs each session's. (This said the aim is "never unshown": it is,
/// under a viewer lent over its pane — review stop 9, F2 — and its reply
/// there rings: Steven, 10-23, "that's a useful signal".)
pub const SHIPPED_ALERTS: &[ShippedAlert] = &[
    ShippedAlert::says("decide", "$decide"),
    ShippedAlert::says("queued", "$queued"),
    ShippedAlert::says("running", "$running"),
    ShippedAlert::says("interrupt", "$interrupt"),
    ShippedAlert::says("hidden", "$hidden"),
    ShippedAlert {
        name: "unshown",
        when: Some("unshown_count"),
        say: Some("$unshown"),
        mark: false,
        bell: false,
        notify: None,
    },
    ShippedAlert {
        name: "unshown_session",
        when: Some("unshown"),
        say: None,
        mark: true,
        bell: true,
        notify: Some("Shannon: news in $session"),
    },
];

/// [`SHIPPED_ALERTS`] as `tui.alert`'s default.
fn shipped_alerts() -> BTreeMap<String, AlertConfig> {
    (SHIPPED_ALERTS.iter())
        .map(|alert| (alert.name.to_string(), alert.config()))
        .collect()
}

/// **`tui.alert` as the files write it, laid over the shipped alerts** — key
/// by key ([`AlertConfig::over`]), so `[tui.alert.decide] when =
/// "decision"` keeps the shipped `say`, and a file naming one alert of its
/// own keeps the shipped ones. The config is folded from the files alone
/// (`ConfigStack::fold`), so without this a file writing any `[tui.alert.*]`
/// would have replaced every shipped alert.
fn alerts_over_shipped<'de, D>(written: D) -> Result<BTreeMap<String, AlertConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let written = BTreeMap::<String, AlertConfig>::deserialize(written)?;
    let mut alerts = shipped_alerts();
    for (name, over) in written {
        let under = alerts.remove(&name).unwrap_or_default();
        alerts.insert(name, over.over(under));
    }
    Ok(alerts)
}

impl Config {
    /// Production entry point: resolve the config path from the
    /// environment, read it, and parse it — falling back to coded
    /// defaults (never crashing) on any problem, with the problem logged.
    ///
    /// Env reads live *here*; the resolution and parsing logic is factored
    /// into pure helpers (`resolve_config_path`, [`Config::parse`]) so
    /// they're testable without touching the real environment or
    /// filesystem.
    pub fn load() -> Self {
        Self::load_reporting().0
    }

    /// [`load`](Self::load), plus what the user needs to be told.
    ///
    /// **A config problem that reaches only the log is a silent failure**, and
    /// this one has teeth: a single TOML typo drops the whole file — including
    /// the user's `[secrets]` paths and names — and the app runs on shipped
    /// defaults, *less protective than the user
    /// believes* — with the only tell in a file they are not reading. `:config
    /// edit` already reports a parse error on screen, so the user learns "it
    /// tells me when my config is bad", then edits in vim outside the app and
    /// is told nothing (Reviewer F).
    ///
    /// The strings are user-facing sentences, not log lines: the caller posts
    /// them through the notice seam. The `warn!`s stay for the log
    /// (`[[feedback_no_silent_failure]]`).
    pub fn load_reporting() -> (Self, Vec<String>) {
        let (_, config, said) = ConfigStack::load_reporting(None, None);
        (config, said)
    }

    /// The resolved config-file path — where `load` reads and `:config edit`
    /// writes. Reads the environment (like `load`); `None` only if neither
    /// `$XDG_CONFIG_HOME` nor `$HOME` is usable.
    pub fn config_path() -> Option<PathBuf> {
        let xdg = std::env::var_os("XDG_CONFIG_HOME");
        let home = std::env::var_os("HOME");
        resolve_config_path(xdg.as_deref(), home.as_deref())
    }

    /// **The API key `shannon setup` saves** — beside the config file
    /// ([`api_key_beside`]), owner-only. Read at launch after
    /// `ANTHROPIC_API_KEY` and `SHANNON_API_KEY_FILE`, and registered as a
    /// credential path whether or not it exists. Reads the environment.
    pub fn api_key_path() -> Option<PathBuf> {
        Some(api_key_beside(&Self::config_path()?))
    }

    /// The machine-wide config, read BEFORE yours at launch: the first
    /// `shannon/config.toml` that exists under `$XDG_CONFIG_DIRS` — Neovim's
    /// rule for `sysinit.vim` (`:h sysinit.vim`: "if nvim/sysinit.vim exists
    /// in one of $XDG_CONFIG_DIRS, it will be used"). `None` when there is
    /// none, which is the normal case.
    pub fn system_config_path() -> Option<PathBuf> {
        let dirs = std::env::var_os("XDG_CONFIG_DIRS");
        resolve_system_config_path(dirs.as_deref(), |p| p.is_file())
    }

    /// Where `secrets.keep_redacted` writes unredacted output:
    /// `$XDG_STATE_HOME/shannon/redacted/`.
    ///
    /// **One resolver, two readers**: the executor writes here and `main.rs`
    /// registers it as a path the gate protects — whether or not the setting
    /// is on, since files from an earlier session outlive the setting. The two
    /// must not compute it separately; a record the model can read is the
    /// whole risk of the feature.
    pub fn redacted_record_dir() -> Option<PathBuf> {
        Some(Self::state_dir()?.join("redacted"))
    }

    /// Where a secret you mark is kept without a password
    /// (`[secrets] password_protect` off, [`FileVault`](crate::FileVault)):
    /// `$XDG_STATE_HOME/shannon/secrets/`.
    ///
    /// **One resolver, two readers**, as [`redacted_record_dir`]'s: launch
    /// opens the store here, and `main.rs` registers it as a path the gate
    /// protects whether or not anything is kept yet.
    ///
    /// [`redacted_record_dir`]: Self::redacted_record_dir
    pub fn kept_secrets_dir() -> Option<PathBuf> {
        Some(Self::state_dir()?.join("secrets"))
    }

    /// The state directory, `$XDG_STATE_HOME/shannon/`: every project's
    /// history, the log, and redacted records live under it. Reads the
    /// environment; `None` only if neither `$XDG_STATE_HOME` nor `$HOME` is
    /// usable.
    pub fn state_dir() -> Option<PathBuf> {
        let xdg = std::env::var_os("XDG_STATE_HOME");
        let home = std::env::var_os("HOME");
        resolve_state_dir(xdg.as_deref(), home.as_deref())
    }

    /// Read + parse + clamp the config at `path`, degrading to built-in
    /// defaults rather than crashing. The testable core of `load`, kept for the
    /// tests that only care about the resulting config.
    #[cfg(test)]
    fn load_path(path: &Path) -> Self {
        Self::load_path_reporting(path).0
    }

    /// [`load_path`](Self::load_path), collecting what the user must be told:
    /// launch with `path` as your config and no system file, so a test reads
    /// a file through the same door launch does. A parse failure says where
    /// it is and never quotes the line, on screen and in the log alike
    /// ([`describe_parse_error`](Self::describe_parse_error)).
    #[cfg(test)]
    fn load_path_reporting(path: &Path) -> (Self, Vec<String>) {
        let (_, config, said) = ConfigStack::load_from(None, Some(path), None, None);
        (config, said)
    }

    /// Parse a TOML document into a `Config`. Pure: no env, no fs. Unknown
    /// *top-level* keys land in `unknown` (warned by `load`); unknown
    /// *nested* keys are rejected by `deny_unknown_fields` and surface as
    /// the `Err`.
    ///
    /// The error is safe to SHOW and not safe to LOG — it quotes the offending
    /// source line, which can itself be a secret. `log_safe_parse_error` is the
    /// form that may be logged.
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// **What a parse failure may say in the log: the message and the line,
    /// never the source text.**
    ///
    /// A `toml::de::Error` renders as a message *plus a quote of the offending
    /// line*, and that line can be a secret: `[[alias]] value = "…"` holds
    /// literals, and a stray token after one makes it the line the parser
    /// points at. `a_parse_error_quotes_the_offending_line` plants a key on
    /// such a line and proves it comes back in the rendered error — and also
    /// pins the blast radius, since the quote is exactly one line, so a typo
    /// *below* a secret never reaches it.
    ///
    /// Three defences, because they fail in different directions:
    ///
    /// - [`message()`](toml::de::Error::message) drops the snippet. Structural,
    ///   so it holds whatever the value looks like — which scrubbing alone does
    ///   not, a low-entropy alias with no issuer prefix being invisible to it.
    /// - serde's *own* type errors quote the value they rejected
    ///   (`invalid type: string "…"`) — content arriving by a second route,
    ///   and the natural mistake puts a password there: `extra = "…"` where
    ///   `extra = ["…"]` belongs. `without_operands` drops that quote, also
    ///   structurally (Reviewer B, 2026-09-29: it reached screen and log).
    /// - What is left is scrubbed anyway.
    ///
    /// The line is counted from the newlines before the span, plus one — the
    /// count of `lines()` is one short when the span starts a line, which is
    /// where every unknown-key error starts (Reviewers B and C).
    ///
    /// Built-in defaults rather than the user's `[secrets]`: this is the
    /// failure to parse the very file those settings live in, so there is no
    /// config to consult, and no alias has been lent this session yet.
    ///
    /// The user still gets the whole error, snippet and all, on screen — it is
    /// their own file and they are not who it is hidden from
    /// (`shannon-secrets`). The log is the copy that gets attached to a bug
    /// report, which is the asymmetry this function exists to make.
    pub fn log_safe_parse_error(err: &toml::de::Error, text: &str) -> (String, usize) {
        // `get` rather than an index: a span start is a token boundary in
        // practice, but slicing a byte offset that is not a char boundary
        // panics, and a config file is user input
        // (`[[feedback_char_boundary_slicing]]`).
        let line = err
            .span()
            .and_then(|s| text.get(..s.start))
            .map_or(0, |before| before.matches('\n').count() + 1);
        let message = crate::secrets::redact(
            &without_operands(err.message()),
            &crate::secrets::SecretsConfig::default(),
            &crate::Aliases::default(),
        )
        .text;
        (with_accepted_form(message), line)
    }

    /// **A parse error as the one line every sink gets**: `line N: <message>`,
    /// the quoted source line dropped and the message scrubbed.
    ///
    /// For the SCREEN as well as the log, because a notice is logged — every
    /// message goes through one door that writes it to the log (`App::post`),
    /// scrubbed by the redactor, which does not catch a low-entropy alias
    /// value. So the careful log-safe form above was defeated one call later:
    /// launch showed "config NOT LOADED — {err}" with the quoted line, and the
    /// notice door wrote it to disk. The row is one line anyway; the line
    /// number says where to look.
    pub fn describe_parse_error(err: &toml::de::Error, text: &str) -> String {
        match Self::log_safe_parse_error(err, text) {
            (message, 0) => message,
            (message, line) => format!("line {line}: {message}"),
        }
    }

    /// Parse *and* clamp — a ready-to-use `Config` from a TOML string, matching
    /// what `load` produces from a file, **plus the same notes `load` shows**
    /// (unknown keys, adjustments; see `review`). For CHECKING an edited config
    /// (`:config edit` saves, and reports what `:reload` would make of the
    /// file). Errors surface (unlike `load`, which degrades) so the caller can
    /// say the file would be refused.
    ///
    /// The notes are part of the return type so a caller cannot drop them by
    /// forgetting to ask — which is how they were lost before.
    pub fn parse_ready(text: &str) -> Result<(Self, Vec<String>), toml::de::Error> {
        let mut config = Self::parse(text)?;
        let notes = config.review();
        Ok((config, notes))
    }

    /// **The config as the TOML table it serializes to** — the one view that
    /// `:set` reads options from, [`changed_keys`](Self::changed_keys) diffs,
    /// and the tests inspect, so no two of them can serialize it differently.
    /// Empty if serializing ever failed, which a `Config` cannot today: every
    /// field is plain data.
    pub fn as_table(&self) -> toml::Table {
        match toml::Value::try_from(self) {
            Ok(toml::Value::Table(t)) => t,
            _ => toml::Table::new(),
        }
    }

    /// **The settings that differ from `self` in `after`**, as dotted keys
    /// (`tui.scrolloff`), in order — what a `:source` or `:unsource` reports
    /// it changed. A table present on one side only is named as the table
    /// (`keymap`, not `keymap.normal.Q`, which this gave as its example:
    /// review stop 12, T9).
    pub fn changed_keys(&self, after: &Config) -> Vec<String> {
        let mut out = Vec::new();
        diff_tables(&self.as_table(), &after.as_table(), "", &mut out);
        out
    }

    /// Clamp out-of-domain numeric knobs into sane bounds, returning a
    /// description of each adjustment for logging. Pure (no env/fs/log) so
    /// it's testable; `load` calls it and warns. `deny_unknown_fields`
    /// catches *typos*, not out-of-range *values* — e.g. a `prompt.height`
    /// of `65535` is a valid `u16` but would starve the output pane (and,
    /// before `split_frame`'s `saturating_add`, overflow). Clamping keeps
    /// the "degrade, don't crash" contract.
    ///
    /// **Each note is a whole sentence carrying its own verb**, because the
    /// notes are not one kind of thing: a value clamped, a value reset, a value
    /// kept but unable to take effect. `load` used to append "— clamped to
    /// range" to every one, so a note whose whole point is that the value was
    /// KEPT said it had been clamped (Reviewers A, B, C, F).
    fn clamp(&mut self) -> Vec<String> {
        let mut adjustments = Vec::new();
        let height = self.tui.prompt.height;
        let clamped = height.clamp(MIN_PROMPT_HEIGHT, MAX_PROMPT_HEIGHT);
        if clamped != height {
            adjustments.push(format!(
                "tui.prompt.height {height} is out of range — using {clamped} \
                 (allowed {MIN_PROMPT_HEIGHT}..={MAX_PROMPT_HEIGHT})"
            ));
            self.tui.prompt.height = clamped;
        }

        // A delay no key can get past: each answer key pressed too soon
        // starts it again, so past a few seconds a call could be answered
        // only by closing its pane — and a project's file can set it
        // (review stop 6, B 4).
        let delay = self.tui.approval_delay_ms;
        if delay > MAX_APPROVAL_DELAY_MS {
            adjustments.push(format!(
                "tui.approval_delay_ms {delay} is longer than {MAX_APPROVAL_DELAY_MS} — using \
                 {MAX_APPROVAL_DELAY_MS}: a key pressed sooner starts the wait again"
            ));
            self.tui.approval_delay_ms = MAX_APPROVAL_DELAY_MS;
        }

        // A decision pane of no rows: its borders, nothing in them, every
        // answer key refused for want of room (review stop 8, B 5). One row
        // is the least — the decision, its keys on the status row.
        if self.tui.approval_max_height == 0 {
            adjustments.push(
                "tui.approval_max_height 0 leaves a decision pane no room — using 1".to_string(),
            );
            self.tui.approval_max_height = 1;
        }

        // **A security control must not have a silent off switch.** TOML 1.0
        // accepts `nan` and `inf` as float literals, and `bits >= f64::NAN` is
        // false for every run — so one typo disables the entire high-entropy
        // pass while `redact_high_entropy = true` still reads as on and nothing
        // says otherwise. Pulled forward from the review because this is the
        // function that reports such things and it is two lines
        // (`[[feedback_no_silent_failure]]`).
        let bits = self.secrets.entropy_bits;
        if !bits.is_finite() || bits < 0.0 {
            let d = super::secrets::default_entropy_bits();
            adjustments.push(format!(
                "secrets.entropy_bits {bits} is not a finite, non-negative number — \
                 using the default {d}"
            ));
            self.secrets.entropy_bits = d;
        }

        // **The same off switch, reached with an ordinary number.** Entropy
        // tops out at log2 of the run alphabet (~6.04 bits by default), so
        // `entropy_bits = 7` disables the pass as surely as `nan` did. Reported
        // rather than reset — it may be deliberate — but in words that say the
        // check is OFF, and INSTEAD of the `entropy_min_run` note below, which
        // would blame the wrong knob (and at 64+ printed a saturated
        // 18446744073709551615).
        let high_entropy = self.secrets.redact && self.secrets.redact_high_entropy;
        let ceiling = super::secrets::entropy_ceiling(&self.secrets);
        let unreachable = high_entropy && self.secrets.entropy_bits > ceiling;
        if unreachable {
            adjustments.push(format!(
                "secrets.entropy_bits {} can never be reached — the high-entropy check is \
                 effectively OFF (a run tops out at {ceiling:.2} bits); use {} or lower, or \
                 set redact_high_entropy = false to say so",
                self.secrets.entropy_bits,
                ceiling.floor()
            ));
        }

        // **An inert setting is worse than a wrong one**, because it looks like
        // a decision. Shannon entropy needs at least 2^bits DISTINCT characters
        // to reach `bits`, so a run shorter than that can never match however
        // random it is, and any `entropy_min_run` below the ceiling does
        // nothing at all. Reported rather than clamped: the user's number is
        // still honoured, they are simply told it cannot bite
        // (`[[feedback_written_rule_no_verb]]` — a rule nobody acts on).
        let floor = self.secrets.entropy_bits.exp2().ceil();
        if high_entropy
            && !unreachable
            && floor.is_finite()
            && (self.secrets.entropy_min_run as f64) < floor
        {
            adjustments.push(format!(
                "secrets.entropy_min_run {} has no effect — at entropy_bits {} nothing \
                 under {} characters can match (kept as written)",
                self.secrets.entropy_min_run, self.secrets.entropy_bits, floor as usize
            ));
        }

        // An exclusion that matches nothing is the same shape one list over:
        // it reads as a decision that took effect. Compared the way the matcher
        // compares (`inert_excludes`), so this cannot disagree with it.
        for entry in super::secrets::inert_excludes(&self.secrets) {
            adjustments.push(format!(
                "secrets.names_exclude \"{entry}\" matches no shipped name — nothing was \
                 excluded (`:config dump` lists them)"
            ));
        }
        adjustments
    }

    /// **Everything the user must be told about a config that parsed** —
    /// unknown top-level keys, then [`clamp`](Self::clamp)'s notes — as
    /// ready-to-show sentences. Mutates only what `clamp` does.
    ///
    /// One function because there are two ways a config arrives and they must
    /// say the same things: `load` at launch, and `:config edit` through
    /// [`parse_ready`](Self::parse_ready). The second discarded everything:
    /// `parse_ready` called `clamp` and dropped its return, and never looked at
    /// unknown keys, so a `nan` was reset without a word and a typo'd section
    /// was ignored while the notice said the config applied (Reviewers A, B, F).
    fn review(&mut self) -> Vec<String> {
        let unknown = self.unknown.keys().map(|key| {
            format!("config: unknown key `{key}` ignored (a typo, or from a newer version)")
        });
        let unknown: Vec<String> = unknown.collect();
        let adjusted = self.clamp().into_iter().map(|a| format!("config: {a}"));
        // (Not the rules: they are read by the program models in force,
        // which a config alone does not know — the app says what they could
        // not honour where it builds the gate from them, `rebuild_rules`.
        // Read here by the shipped set, a rule only a user's model refuses
        // went unsaid, and one only the shipped set refuses was said refused
        // while in force; review 2026-10-11, A and C.)
        let unread = self
            .secrets
            .unhonoured()
            .into_iter()
            .map(|u| format!("config: {u}"));
        let retired = self.secrets.keep_marked.map(|_| {
            "config: `[secrets] keep_marked` is retired — a secret you mark is always kept, \
             and shown as itself when its conversation reopens; `password_protect` decides \
             whether that asks for your password"
                .to_string()
        });
        (unknown.into_iter())
            .chain(adjusted)
            .chain(unread)
            .chain(retired)
            .collect()
    }
}

/// Resolve the config-file path from environment values, without reading
/// the environment itself (callers pass the values in — that's what keeps
/// this testable). `$XDG_CONFIG_HOME/shannon/config.toml`, falling back to
/// `$HOME/.config/shannon/config.toml`. `None` only if neither is usable.
///
/// Per the XDG spec, `$XDG_CONFIG_HOME` is honored only when set to a
/// non-empty *absolute* path; an empty or relative value is "invalid" and
/// falls back to `$HOME/.config` — so we don't accidentally read a stray
/// `./shannon/config.toml` from whatever directory we were launched in.
fn resolve_config_path(xdg: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    Some(
        xdg_base(xdg, home, Path::new(".config"))?
            .join(DIR_NAME)
            .join("config.toml"),
    )
}

/// The name Shannon's directories go by, under each XDG base.
pub const DIR_NAME: &str = "shannon";

/// **Where the saved API key lives, given the config file** — the one rule
/// both launch (`Config::api_key_path`, which reads the key) and setup (which
/// writes it, from the config path the app was handed) apply, so the two
/// cannot name different files.
pub fn api_key_beside(config: &Path) -> PathBuf {
    config.with_file_name("api_key")
}

/// The variable holding a key itself.
pub const API_KEY_VAR: &str = "ANTHROPIC_API_KEY";

/// The variable that names a key FILE.
pub const KEY_FILE_VAR: &str = "SHANNON_API_KEY_FILE";

/// **Every variable that carries Shannon's key or names its file** — what
/// launch reads (`shannon-tui`'s `keys`), and what a process that is not
/// Shannon's is never handed (`backend::claude_code`). (The pre-rename name
/// of the file variable was read until 11-08, when no machine set it.)
pub const KEY_VARS: [&str; 2] = [API_KEY_VAR, KEY_FILE_VAR];

/// Collect the dotted keys whose values differ between `a` and `b` into
/// `out`: tables recurse, and anything else that differs — present on one
/// side only included — is one key.
fn diff_tables(a: &toml::Table, b: &toml::Table, prefix: &str, out: &mut Vec<String>) {
    let keys: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    for key in keys {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match (a.get(key), b.get(key)) {
            (Some(toml::Value::Table(x)), Some(toml::Value::Table(y))) => {
                diff_tables(x, y, &path, out);
            }
            (x, y) if x != y => out.push(path),
            _ => {}
        }
    }
}

/// **A deserialization message without the value it rejected** — serde quotes
/// it (`invalid type: string "…", expected a sequence`; `` unknown variant `…` ``),
/// and in a config file that value can be a credential in the wrong shape.
/// The shape stays — "a string where a list belongs" is what says what to
/// fix — and the operand goes. See [`Config::log_safe_parse_error`].
fn without_operands(message: &str) -> String {
    // (label kept, opening delimiter, closing delimiter)
    const QUOTED: [(&str, &str, char); 2] = [("string", " \"", '"'), ("variant", " `", '`')];
    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    while let Some((at, label, open, close)) = QUOTED
        .iter()
        .filter_map(|&(label, open, close)| {
            let marker = format!("{label}{open}");
            rest.find(&marker).map(|at| (at, label, open, close))
        })
        .min_by_key(|&(at, ..)| at)
    {
        out.push_str(&rest[..at + label.len()]);
        let quoted = &rest[at + label.len() + open.len()..];
        // Past the closing delimiter; a backslash escapes the next char (serde
        // Debug-formats a string, so an inner `"` arrives as `\"`).
        let mut chars = quoted.char_indices();
        let mut end = quoted.len();
        while let Some((i, c)) = chars.next() {
            if c == '\\' {
                chars.next();
            } else if c == close {
                end = i + c.len_utf8();
                break;
            }
        }
        rest = &quoted[end..];
    }
    out.push_str(rest);
    out
}

/// **A parse error, plus the form that IS accepted, where toml's own words
/// only say what is not** (Steven, 2026-10-03: rejecting a duplicate is fine,
/// "just we should specify which is accepted"). The natural duplicate is a
/// second `rules = [...]` line for one more rule.
fn with_accepted_form(message: String) -> String {
    if message.starts_with("duplicate key") {
        format!(
            "{message} — a key is written once in a table; a list takes every entry in \
             one `[...]`, which may span lines"
        )
    } else if message.starts_with("invalid array") {
        // `rules = [allow ls]` — the rules read like words, so they get
        // written as words (Steven's own sketch did).
        format!(
            "{message} — a list is `[\"one\", \"two\"]`: each entry quoted (`'…'` for a \
             regex), a comma between them; it may span lines"
        )
    } else {
        message
    }
}

/// **The settings that guard something**, as dotted-key prefixes: what a file
/// that is not yours ([`Origin::is_yours`]) is called out for changing.
///
/// Whole sections where it can be, so a key added to `[secrets]` is covered
/// the day it exists. And deliberately NOT a judgement of whether a change
/// weakens anything: telling "stricter" from "looser" needs a reader (Steven:
/// keep it gentle — anything smarter wants a model or human eyes). It names
/// what changed; you judge. The manual's list is held to this one by
/// `the_safety_list_is_the_manuals`.
///
/// `continue` and `merge` are text a file writes and the MODEL is sent (the
/// `:continue` instruction goes as a user turn); `keymap` decides what your
/// keys do — each is something a project file must not change quietly
/// (Reviewer B, 2026-09-29).
pub const SAFETY_KEYS: &[&str] = &[
    "secrets",
    "permissions",
    "exec",
    "agents",
    "alias",
    "continue",
    "merge",
    "keymap",
    "tui.confirm_delete",
    // At 0 a double-tap answers the next call unread.
    "tui.approval_delay_ms",
    // What the model is told, and where it writes: a project's file adding a
    // location is said so (Steven, 10-06: any config file may).
    "memory",
    "skills",
    // What rings and what leaves the terminal for the OS's notifications —
    // a project's file may turn back on what you turned off, and wins, said
    // so (review stop 9, E1; Steven, 10-24: "The project file should win but
    // we can surface conflicts"). Whole, so an alert of its own is named.
    "tui.alerts",
    "tui.alert",
];

/// **The settings that hold credentials verbatim**, as dotted-key prefixes:
/// `secrets.extra` is documented as the place to paste "a database password,
/// an internal token", and an `[[alias]] value` is a literal.
///
/// They go only where a secret is already expected — the config file, which
/// `:config edit` writes owner-only. So `:set` refuses them (typed there, the
/// value lands in command history, the notice and the log;
/// [`set::plan`]). Not yet wired: `:mkconfig`,
/// which must leave them out of anything it writes into a project.
pub const CREDENTIAL_KEYS: &[&str] = &["secrets.extra", "alias"];

/// Is the dotted `key` the setting `prefix` names, or inside it? `tui.mouse`
/// is under `tui` and under `tui.mouse`; it is not under `tui.mou`.
pub fn key_is_under(key: &str, prefix: &str) -> bool {
    key.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

/// [`Config::system_config_path`] without the environment: the first
/// `<dir>/shannon/config.toml` that `exists`, over the colon-separated `dirs`
/// — `/etc/xdg` when unset or empty, the XDG spec's default. A relative entry
/// is skipped, as the spec requires, for the reason `resolve_config_path`
/// gives: a relative path would read whatever directory we were launched in.
fn resolve_system_config_path(
    dirs: Option<&OsStr>,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let dirs = dirs
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| OsStr::new("/etc/xdg"));
    std::env::split_paths(dirs)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(DIR_NAME).join("config.toml"))
        .find(|p| exists(p))
}

/// The XDG base dir for a kind of data: `$<XDG var>` when it is a non-empty
/// **absolute** path (per the XDG spec), else `$HOME/<home_sub>`. The shared
/// core of [`resolve_config_path`] and [`resolve_state_dir`] — the is-absolute
/// rule is the subtle, spec-load-bearing part, kept in exactly one place so the
/// two resolvers can't drift. `None` if neither source is usable.
fn xdg_base(xdg: Option<&OsStr>, home: Option<&OsStr>, home_sub: &Path) -> Option<PathBuf> {
    // `HOME` held to what `secrets::home_dir` holds it to: an empty or
    // relative one is no home, or the config, its program models and the
    // store resolve under the launch directory — the project (review
    // 2026-10-11, E2: `HOME=""` gave `.config/shannon/config.toml`).
    xdg.map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|h| h.is_absolute() && h.parent().is_some())
                .map(|h| h.join(home_sub))
        })
}

/// Resolve the base **state** directory (where conversation history is
/// persisted) from environment values, without reading the environment itself —
/// the same testable pattern as `resolve_config_path`, but for the XDG *state*
/// dir: `$XDG_STATE_HOME/shannon`, falling back to `$HOME/.local/state/shannon`.
/// As with config, `$XDG_STATE_HOME` is honored only when set to a non-empty
/// absolute path; an empty or relative value falls back to `$HOME`, so we never
/// scatter a `./shannon` history dir into the launch directory. `None` only if
/// neither is usable (the caller then runs with ephemeral, in-memory history).
pub fn resolve_state_dir(xdg_state: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    Some(xdg_base(xdg_state, home, Path::new(".local/state"))?.join(DIR_NAME))
}

/// The fully-defaulted reference document, printed by `:config dump`
/// (M2 command work). Every value here equals the coded default; a test
/// asserts it round-trips to `Config::default()` *and* that it is
/// complete (no field missing), so the dump can never drift from the
/// struct.
pub const DEFAULT_TOML: &str = "\
# Shannon Code configuration.
#
# This is the FULLY-DEFAULTED reference: every value shown is exactly what
# the program already uses when the key is absent. Your real config at
# `$XDG_CONFIG_HOME/shannon/config.toml` should contain ONLY the keys you
# want to change -- it is read as a sparse diff over these defaults, and
# nothing but you writes it (:config edit saves what you wrote).

[tui]
# The interface: the mouse and the arrows, the clipboard, splits and search,
# scrolling, the approval pane, how a command is laid out, colours and the
# cursor. The sections after it are the interface's too.

# Capture the mouse: click focuses a pane, the wheel scrolls the pane under the
# cursor, drag selects. Set to false to leave the mouse to the terminal. Even
# when true, Shift-drag falls through to the terminal's native selection.
mouse = true
# Whether the arrow keys move, in every pane, panel and float. Off, an arrow
# says why and does nothing -- h j k l only (vim: noremap <Up> <Nop>). The :,
# / and ? lines keep them (they recall history and move the cursor, and no
# other key does), and so does a screen that covers the frame, such as setup.
arrows = true
# Whether a y also copies to the system clipboard (vim: clipboard=unnamedplus).
# Off, a yank fills only the register p pastes from, as vim's default does.
copy_on_yank = true
# Whether releasing a mouse drag copies what it selected to the system
# clipboard, as a terminal's own selection does (vim: clipboard=autoselect).
# Off: a drag selects, and y copies. On, the selection stays up for y, d or :,
# and p's register is untouched.
copy_on_select = false
# What a copy to the clipboard (y, or a drag) carries for a marked secret it
# touches, whole or in part: \"value\" as the screen shows it (yours to have --
# the model never sees it: printing the clipboard asks first, and a held value
# comes back as its handle), \"handle\" its handle instead, or \"skip\" to copy
# nothing and say so. p's register is never changed.
copy_secrets = \"value\"
# The model NEW sessions start on: a short name (haiku, sonnet, opus, fable) or
# a full model id. Empty means the build's default, sonnet. A starting value,
# not a lock -- :model changes it per session, which is what lets two panes run
# two different models and answer the same prompt side by side.
model = \"\"
# What a launch does with the previous session: \"fresh\" starts a new empty
# session (the previous one stays on disk, browsable via :graph); \"last\"
# resumes where you left off.
resume = \"fresh\"
# How :graph orders sibling sessions: \"recency\" (most-recently-active first) or
# \"hash\" (by content id -- stable but arbitrary).
graph_sort = \"recency\"
# :graph visualization: \"git\" (git log --graph flavored: per-branch-colored
# commit dots, (HEAD)/branch decoration, compact one-line) or \"tree\" (indented
# collapsible tree). Both collapsible.
graph_style = \"git\"
# Glyphs for the git lane graph: \"unicode\" (● │ ├ ╰) or \"ascii\" (* | + \\) for
# terminals without good Unicode.
graph_chars = \"unicode\"
# Where :graph shows a NAMED session's id: \"after\" its name (auth-fix 1a2b3c4d)
# or \"hidden\" (auth-fix; the inspector still shows it). A session with no name,
# and a turn, always show theirs -- they have nothing else to be called by.
graph_ids = \"after\"
# Ask y/n before a destructive delete (:delete, :delete all, :delete <hash>, and
# d in :graph). Set false to delete immediately.
confirm_delete = true
# Where a split puts the NEW pane (vim's `splitbelow` / `splitright`). Both off
# by default, matching vim: `:split` opens above, `:vsplit` opens to the left.
# On, `:split` opens below.
splitbelow = false
# On, `:vsplit` opens to the right.
splitright = false
# How a search (/ ? n N * #) treats case (vim's `ignorecase` / `smartcase`).
# Both off by default, matching vim: case is matched. ignorecase ignores it;
# smartcase, with it on, matches case again for a pattern you type with a
# capital in it (/mara finds Mara, /Mara only Mara). \\c or \\C in a pattern
# decides for it.
# On, case is ignored.
ignorecase = false
# On, with ignorecase, a capital in the pattern matches case again.
smartcase = false
# Rows of context kept visible above/below the cursor before a window scrolls
# (vim's scrolloff) -- in every window: the prompt, the transcript, :graph and
# every page. 0 = vim's default feel; large values keep the cursor centered.
scrolloff = 5
# Upper bound on the approval pane's height, in content rows. The pane sizes
# itself to its content -- a short report gets a short box -- so this only caps
# how much of the transcript a long one may borrow.
approval_max_height = 18
# How long a call in that pane waits before a key answers it, in milliseconds:
# from when it is shown, and again from each answer key pressed too soon, so a
# double-tap or a sticky key never answers the next call unread. 0 = at once.
approval_delay_ms = 300
# Pretty-print shell commands recorded in the transcript (control flow on its
# own indented lines, as the approval gate shows them). false keeps them
# verbatim.
pretty_commands = true
# Past this many columns a command is laid out across lines -- a chain at its
# &&/||, a pipeline at its |, a command before a run of its flags (packed to
# this width, each flag kept with its value). 0 never breaks for length.
command_width = 72
# At most this many flags on a line of a command broken for length; 1 puts each
# on its own line. 0 = as many as fit in command_width.
flags_per_line = 0
# What :set says after it changes an option: \"default\" (the value you got,
# adjusted if the config had to), \"quiet\" (nothing -- vim's :set), or
# \"verbose\" (the value and every layer that sets it, as :verbose set x? shows).
set_report = \"default\"
# What every colour looks like. \"terminal\" names terminal colours -- keyword
# magenta, a focused border cyan -- so your terminal's palette decides them.
# Shipped too: catppuccin-mocha, tokyonight-night, gruvbox-dark, nord, dracula,
# solarized-dark, rose-pine, kanagawa-wave and more (:help themes). A
# file themes/<name>.toml beside this one is yours, and comes first. :reload
# or :set applies it to what is on screen.
theme = \"terminal\"
# Where else themes are found: directories of terminal palettes (a ghostty or
# iTerm2 theme file -- sixteen colours and a ground), each a theme by its file's
# name, as theme = \"Catppuccin Mocha\". \"ghostty\" is ghostty's own theme
# directories, wherever this machine keeps them. Only colours are read.
theme_import = [\"ghostty\"]
# The colours the status line's ctx takes as the context window fills:
# \"percent:colour\" stops, each colour from its percent up -- a terminal colour's
# name, its number, or #rrggbb. Two stops are thresholds; many make a stepped
# gradient. Below the first stop, the row's own colour.
ctx_colors = [\"70:yellow\", \"90:red\"]
# The cursor's shape in each mode: neovim's 'guicursor', read by neovim's
# rules -- comma-joined parts, each modes:arguments. Here a block in Normal,
# Visual and at the end of the : line; a bar in Insert and inside the : line;
# an underline while an operator (d) or r waits. To blink as well:
# :set tui.guicursor+=a:blinkon500-blinkoff500. \"\" leaves the terminal's
# own shape. Every mode and argument: :help cursor.
guicursor = \"n-v-c-sm:block,i-ci-ve:ver25,r-cr-o:hor20\"
# How a time is shown: \"locale\" (your locale's date and time), \"unix\" (the
# seconds it is saved as), or a strftime pattern such as \"%Y-%m-%d %H:%M\".
time_format = \"locale\"

[tui.graph_glyphs]
# :graph node glyphs (optional) -- a sparse diff over the set graph_chars picks,
# one SINGLE character per role. Roles and their unicode/ascii defaults:
#   session = \"\\u25cf\" / \"*\"   root + branch session heads     (U+25CF ●)
#   child   = \"\\u25c6\" / \"o\"   spawned child sessions (:child) (U+25C6 ◆)
#   clone   = \"\\u25cf\" / \"*\"   clones
#   merge   = \"\\u25cf\" / \"*\"   merge/squash compositions
#   running = \"\\u25b6\" / \">\"   the busy marker on running rows  (U+25B6 ▶)
#   held    = \"\\u2016\" / \"!\"   a child stopped, waiting for you  (U+2016 ‖)
#   turn    = \"\\u25e6\" / \".\"   a turn's row                     (U+25E6 ◦)
#   compacted = \"\\u229a\" / \"@\" a summary: where a compaction was (U+229A ⊚)
#   claude  = \"\\u25c7\" / \"+\"   a Claude Code conversation       (U+25C7 ◇)
#   continued = \"\\u29d3\" / \"X\" one a Shannon session goes on from (U+29D3 ⧓)
# Their colours are the theme's (ui.graph.continued, ...; :help themes).
# Empty by default; an example (uncomment to apply):
# child = \"\\u25c7\"    # hollow diamond for children instead

[tui.prompt]
# The prompt pane, where you write what you send: how tall it is, and the mode
# it starts in.

# Visible editable rows in the prompt pane (excludes the border).
height = 10
# Mode the prompt starts in: \"normal\" or \"insert\".
start_mode = \"normal\"

[tui.fold]
# Which of a transcript's blocks have a fold, and how it starts, by kind
# (:help folds): \"shut\", \"open\", \"manual\" -- none until zc makes
# one -- or { shut_over = N }, shut when it holds more than N lines. zo
# opens one and zc shuts it, zR and zM every fold there is: your window's,
# for this run.

# What the model thought.
thinking = \"shut\"
# A tool call's record.
tool = \"shut\"
# The model's reply.
reply = \"manual\"
# Your message.
message = \"manual\"

[tui.status]
# What the status row says, per kind of pane (:help statusline).
# $name is a value -- $mode, $pane, $model, $ctx, $tokens_in, and the rest
# the manual lists; a { ... } group shows only when a value in it does, and
# when the row is too narrow the rightmost group goes first. $alerts is what
# waits on you, and a group showing it is cut, never dropped.
# [text](style) colours text; a [ or ( not written that way is just text.
# $fill is the room the rest leaves, so what follows sits at the right:
# ${alert.hidden} there, a decision no pane shows, which $alerts then
# leaves out. $readonly is `read-only` where no text can be typed; no row
# here says it, since typing there says so, as vim's E21 does.

# The prompt's row.
prompt = '[$mode] $pane{ · $alerts}{ · $activity}{ · $model}{ · ctx $ctx}{ · tok ↑$tokens_in{ ($cache)} ↓$tokens_out}{ $fill${alert.hidden}}'
# A transcript's row.
output = '[$mode] $pane{ · $alerts}{ · $activity}{ · $model}{ · ctx $ctx}{ · tok ↑$tokens_in{ ($cache)} ↓$tokens_out}{ $fill${alert.hidden}}'
# A decision pane's row.
decision = '[$mode] $pane{ · $alerts}{ · $activity}{ · help `:help $help`}{ $fill${alert.hidden}}'
# :graph's row.
graph = '[$mode] $pane · $position{ · $alerts}{ · $activity}{ · help `:help $help`}{ $fill${alert.hidden}}'
# Every read-only page: :help, :rules, :ls, :messages, the inspector, ...
page = '[$mode] $pane · $position{ · $alerts}{ · help `:help $help`}{ $fill${alert.hidden}}'
# How a token count is written: \"short\" (10.4M, 40k) or \"exact\" (10415096).
tokens = \"short\"

[tui.alerts]
# Every alert at once (:help alerts): which are drawn nowhere, what a mark
# draws, and whether any may ring the bell or send a notification.

# Alerts not drawn, by name: a shipped one, or one of yours.
disabled = []
# What an alert's `mark` draws beside a session in :ls.
glyph = \"🔔\"
# Every alert's bell: false rings none, whatever an alert asks.
bell = true
# Every alert's desktop notification: false sends none.
notify = true
# How a notification reaches the terminal: \"osc9\" (ghostty, iTerm2, WezTerm),
# or \"tmux\", wrapped for tmux to pass on -- which it does only with
# `set -g allow-passthrough on` in your tmux.conf.
notify_via = \"osc9\"

# What waits on you, each a rule (:help alerts): `when` it holds -- a
# fact, or a fact compared with a whole number, ctx_percent >= 80 -- and what
# the status row `say`s of it, where $alerts or ${alert.<name>} is written.
# The first five shipped have no `when`: each holds whenever its value has
# something to say. A table of yours with a shipped one's name changes only
# what it names.

[tui.alert.decide]
# A decision whose pane cannot show its keys: DECIDE: d deny · a allow.

# What the row says while it holds.
say = '$decide'

[tui.alert.queued]
# Prompts waiting to be sent: 2 queued.

# What the row says while it holds.
say = '$queued'

[tui.alert.running]
# An allowed command, running: running: cargo test.

# What the row says while it holds.
say = '$running'

[tui.alert.interrupt]
# ^C:stop, while anything runs that CTRL-C would stop.

# What the row says while it holds.
say = '$interrupt'

[tui.alert.hidden]
# Decisions waiting where you cannot answer them here -- hidden by :hide, or
# in a session no window shows (decision hidden · :decide 3), or in a pane in
# another tab (DECIDE in tab 2 · 2gt).

# What the row says while it holds.
say = '$hidden'

[tui.alert.unshown]
# News you have not seen: a session changed where no pane drew it, and nothing
# runs in it now. Said on the row: how many sessions have it, and the way to
# the first (news in 2 sessions · :b 3).

# It holds while any session has news unseen.
when = 'unshown_count'
# What the row says while it holds.
say = '$unshown'

[tui.alert.unshown_session]
# The same news, a session at a time: marked beside each in :ls, and for each,
# the bell and a notification as it arrives. A notification's words may hold
# $alert (the alert's name) and $session (b3 and its name).

# It holds for each session with news unseen.
when = 'unshown'
# Its mark beside each such session in :ls.
mark = true
# The bell as the news arrives.
bell = true
# A notification as the news arrives, in these words.
notify = 'Shannon: news in $session'

# An alert of your own (uncomment to apply), its border on the windows of each
# session it holds for, and the glyph beside each in :ls:
# [tui.alert.ctx]
# when = 'ctx_percent >= 80'
# say = '[ctx $ctx](bold fg:red)'
# border = 'ui.caution'
# mark = true

[merge]
# Distilling a history into a summary: what `S` in :graph (:distill) and
# :compact ask the model for, and whether you review it first (:help compact).

# The instruction sent to the model when `S` (:distill, in :graph) or :compact
# asks it to distill a branch, a marked turn range or a session's history into
# a summary. Each is reviewed in $EDITOR before it composes only as
# distill_review and compact_review say. Words after :compact are added to it
# for that one summary, as /compact takes them.
# First person: the summary lands as the assistant's message in the history.
squash_instruction = \"\"\"Summarize this conversation into one \\
concise report of what was done, decided, and learned, in the assistant's first \\
person. Preserve any paths, commands, code, or identifiers needed to continue the \\
work. Output only the summary.\"\"\"
# How many of a session's last turns :compact keeps as they were; the rest
# become one summary, asked with squash_instruction. 0: the whole history, as
# Claude Code's /compact summarises it.
compact_keep = 0
# Whether :compact's summary opens in $EDITOR before it replaces anything.
# false (as /compact): it replaces them when it is written. `:compact!` does
# the other for once.
compact_review = false
# Whether `S`'s summary (:distill) opens in $EDITOR before it composes. false,
# as compaction: it composes when it is written — a branch's or gathered
# summary beside its sources, in no pane, until you show it (:b N). `:distill!`
# does the other for once.
distill_review = false

[continue]
# How :continue resumes a truncated (max_tokens) or interrupted turn
# (:help continue).

# \"instruct\" (default) sends the instruction below as a normal follow-up --
# works on every model; the resumed text arrives as a new turn (squash the
# pair afterwards if you want one seamless turn). \"prefill\" re-sends the
# transcript ending with the partial and the model continues it in place --
# an exact splice composed back into ONE turn, but the 4.6+/5-family models
# reject it (HTTP 400); haiku-class models accept it.
strategy = \"instruct\"
# The follow-up the instruct strategy sends; {tail} becomes the end of the
# truncated reply.
instruction = \"\"\"Your previous response was \\
interrupted and ended with:\\n\\n{tail}\\n\\nContinue exactly from where you left \\
off -- do not repeat or summarize what you already wrote; pick up mid-sentence \\
if that is where it stopped.\"\"\"

[agents]
# Child sessions the model spawns: how deep and how wide, and what happens at
# a limit (:help agents).

# Only spawn (tool-call) edges count -- your own branches/clones never do.
# How deep spawn chains recurse: a session that deep may not start another
# child; 0 gates every spawn.
max_depth = 3
# How many children one session may spawn.
max_fanout = 4
# At a limit: \"confirm\" asks y/N with the reason, \"block\" refuses, \"advisory\"
# spawns anyway and just tells you.
on_limit = \"confirm\"

[turn]
# What one turn may spend, and how hard the model works on it: the reply's
# ceiling, the context you work inside, effort and thinking (:help cost).

# Ceiling on a single reply, in tokens. 0 means the model's own maximum, which
# is the default: Shannon imposes no limit the provider would not. Set a number
# if you want a hard bound on what one answer can cost -- a truncated reply
# says so on the status line and `:continue` resumes it.
max_output_tokens = 0
# The context budget you want to work inside, in tokens. 0 means the model's
# own window (1M on Opus/Sonnet/Fable 5, 200k on Haiku 4.5). Set a number to
# work inside a smaller one -- a long context costs more on every later turn,
# and gets less reliable well before it gets full. This moves the `ctx` gauge
# and its warning; it cannot make a model accept more than it accepts.
max_context_tokens = 0
# The DEFAULT effort -- how much work the model puts into a reply: low, medium,
# high, xhigh, max -- for a session that has not chosen its own with `:effort`
# (a session's effort is kept with it, as its model is). \"default\" sends
# none: through the API the model works at its own (high on Sonnet 5.5 and
# Fable 5.1, medium on Opus 5.5; Haiku 4.5 takes none), through Claude Code
# at your Claude Code setting. Higher thinks more and costs more. A model that
# does not take the level is sent none. `:set effort=<level>` sets this for
# the run, and so does CLAUDE_CODE_EFFORT_LEVEL at launch (over this line).
effort = \"default\"
# What a model that thinks adaptively (4.6 and later) shows of its thinking:
# \"summarized\" (a summary of its reasoning and its notes between tool calls),
# \"updates\" (the notes alone) or \"omitted\" (nothing -- a long tool-using
# turn goes quiet). Thinking happens and is billed the same under all three.
thinking = \"summarized\"
# Thinking for a model that thinks only on a budget (4.5 and earlier, such as
# Haiku 4.5), in tokens: 0 asks for none, as those models do unasked. The API
# takes 1024 at least, and less than the reply's ceiling.
thinking_budget = 0

[cache]
# Prompt caching (:help cost).

# On by default: a prefix written once and read once already costs less than
# sending it twice, and a prompt too short to cache is simply processed
# uncached -- no error, no surcharge. Set false if you would rather your spend
# have exactly one shape.
enabled = true
# How long an entry lives: \"5m\" (the default) or \"1h\". A read restarts the
# clock, so an active conversation stays warm either way; \"1h\" is for coming
# back to a prefix much later, and its writes cost 2x base input instead of
# 1.25x.
ttl = \"5m\"

[exec]
# How an APPROVED shell command is run (:help exec). There is no sandbox: your
# approval is the boundary, and the command runs as you, in your shell, in your
# working directory -- but with a MINIMAL environment (PATH, HOME, SHELL, TERM,
# locale, TZ, PWD), not yours. Nothing else is inherited, so an approved command
# does not see your credentials -- and does not see CARGO_HOME, SSH_AUTH_SOCK or
# your proxy variables either.

# Seconds one attempt may run before it is killed.
timeout_secs = 120
# Extra attempts after a failure. 0 = off (the default), -1 = unlimited.
# Off on purpose: a shell command is not assumed idempotent, and re-running one
# that TIMED OUT is the riskiest case -- the timeout says it did not finish, not
# that it did nothing, so a retry can compound a partial effect. Turn it on only
# for commands you know are safe to repeat.
max_retries = 0
# Cap on captured output, in bytes.
max_output_bytes = 65536
# How much of a tool call is written to the LOG FILE (separate from the
# transcript, which always shows what happened in this session):
#   \"off\"       -- nothing
#   \"decisions\" -- the command, the verdict, and how you answered (default)
#   \"full\"      -- also the command's output
# Output is opt-in because it is the part most likely to contain secrets, and
# a log is a durable artifact.
audit = \"decisions\"
# The shell that interprets the command. Keep this bash-compatible: the
# permission analyzer models bash (process substitution `<(cmd)` is a bash-ism),
# so a shell with different semantics would mean the analysis you approved is
# not a description of what runs.
shell = \"bash\"
# Variables a command is given beyond the minimal set, as NAME = \"value\"; a
# $NAME in a value is read from Shannon's own environment. PATH set here is the
# one commands run with (yours, unset); Claude Code's process gets them too:
#   env = { PATH = \"$HOME/.cargo/bin:$PATH\", SSH_AUTH_SOCK = \"$SSH_AUTH_SOCK\" }
# Shannon's own key variables, and those [secrets] env lends, are never handed
# on ([secrets] gives a command a value), and a PATH entry that is relative or
# empty is said: a file found there runs under the name your rules allow. Only
# your own config may set it.
env = {}
# Variables NO command is handed -- Shannon's own or Claude Code's -- whatever
# else names them; a project's config may add to this, never take from it:
#   withhold = [\"AWS_SECRET_ACCESS_KEY\", \"OPENAI_API_KEY\"]
withhold = []

[files]
# Shannon's own file tools -- Read, Edit, Write (:help files).

# What an Edit, or a Write over a file that exists, asks the model to have
# read of it first: \"any\" -- some of it (the default; a partial read
# passes), \"whole\" -- every line, \"off\" -- nothing. What it has read is
# what the conversation it was sent still holds, of the file as it is now:
# a file changed since, by anyone, is unread again.
read_first = \"any\"

[secrets]
# Keeping credentials from the model (:help secrets): what is scrubbed from
# output, the secrets you mark, the paths that commonly hold credentials, and
# values lent to commands.

# Scrub credentials from command output BEFORE it is sent to the model or
# written to the episode log. On by default: a false negative costs you a key
# in someone else's retention window; a false positive costs you mangled
# SOURCE the model then works from, which is why the rules below are tuned for
# precision. See docs/DesignDocs/secrets.md.
redact = true
# Keep an UNREDACTED copy of any output that was redacted, one owner-only file
# per command under $XDG_STATE_HOME/shannon/redacted/ -- for a command that
# prints a new secret once, which is otherwise gone (it is scrubbed before
# anything records it). OFF by default: it is a plaintext file of your secrets
# on disk, not encrypted. The notice tells you the path; the model is never
# told it, and reading that directory always asks.
keep_redacted = false
# Password-protect conversations that hold a secret you marked (:secret). A
# mark is kept for later runs either way, in Shannon's own owner-only store
# under $XDG_STATE_HOME/shannon/secrets/, and a reopened conversation shows you
# its value; the model only ever has the handle. OFF by default: nothing asks.
# On: secrets are sealed under a passphrase Shannon asks for before such a
# conversation opens -- not given, it does not open; forgotten, the sealed
# secrets are lost. Set here, not with :set (it re-stores secrets on disk).
# NOT BUILT YET: today this changes nothing.
password_protect = false
# Also scrub long high-entropy strings matching no known issuer prefix. Catches
# formats nobody has published, at the price of occasionally redacting a hash
# or a base64 blob.
redact_high_entropy = true
# The three knobs of that rule. A credential is ONE unbroken run; a name is
# structured -- so the unit measured is a maximal run of letters and digits plus
# the characters below, and a dot, slash or space ends the run.
#
# Characters that belong to a run rather than ending it. '-' and '_' are here
# because URL-safe base64 embeds them. '/' is deliberately absent: with it, a
# whole path fuses into one run and gets redacted. Empty it for alphanumeric
# runs only; widen it if your secrets carry a separator we don't expect.
entropy_token_extra = \"+=_-\"
# How long a run must be before entropy is consulted at all. NOT a tuning knob
# so much as an arithmetic floor: entropy needs at least 2^bits DISTINCT
# characters to reach `bits`, so at entropy_bits = 4.5 a run shorter than 23 can
# never match and anything lower here is inert. Measured over 905k runs of real
# text and file paths, dropping this from 32 to 1 adds zero false positives --
# while 32 excludes the 23-31 band outright. Recall inside that band is low
# anyway (~5% of random 24-char tokens reach 4.5 bits). Raise it together with
# entropy_bits, not alone.
entropy_min_run = 23
# Shannon bits per character a run must reach. This is the real precision/recall
# dial, and it has no right answer -- it depends on what your output looks like.
# Measured on this project (false positives out of 905k real runs, vs the share
# of random tokens caught at each length):
#
#   bits   FPs   L24   L32   L40   L64
#   4.35     8   19%   92%  100%  100%
#   4.50     1    7%   70%   98%  100%     <- default
#
# The false positives that appear below 4.5 are content-hashed asset names like
# `FiraMono-Regular-87c26294` and dated filenames like
# `SECURITY-2026-07-27-redirect-bypass`. Lower it if you would rather
# over-redact than miss a 32-character key; raise it if your work is full of
# hashes.
# Entropy cannot exceed log2(n), so a 32-char run tops out at 5.0 -- which is
# why 4.5 is a demanding bar at that length and an easy one at 64.
entropy_bits = 4.5
# How long a VALUE must be before a credential-ish NAME is enough to redact it.
# The delimiters the name rule keys on (= : >) are ordinary programming syntax,
# so name-alone redacted 1 line in 677 of this project's Rust -- turning
# `let token = &argv[i];` into `let token = &[redacted: named secret][i];`.
# A 12-char floor removes ~95% of that. It does not apply where the context
# binds a credential -- an assignment with no spaces to an environment-style
# name or a flag (PGPASSWORD=x, --password=x), or a quoted value bound to a
# password's name -- since a short password is still a password; it gives up
# a short value where the text could be code (password: x, unquoted). Shape
# detection is unaffected: a known issuer prefix still redacts at any length.
name_min_value = 12
# Shipped credential-hint names to STOP using, for a codebase where one of them
# is ordinary vocabulary. `names` only adds; this is how you narrow.
# For example: names_exclude = [\"token\"]
names_exclude = []
# Literal values to scrub verbatim -- for secrets whose shape can't be guessed
# (a database password, an internal token). Values shorter than 8 characters
# are ignored rather than blanking your output. Only here: `:set` refuses it,
# since a value typed there is echoed, logged and kept in history.
# For example: extra = [\"my-database-password\"]
extra = []
# Additional credential PREFIXES to recognise, beyond the shipped set
# (Anthropic, OpenAI, GitHub, GitLab, AWS, Slack, Google, HuggingFace, npm,
# PyPI, ...). The shipped list is a seed, not the list: an internal token
# format is a shape we have never heard of, and we would silently not protect
# it.
# For example: prefixes = [\"acme_tok_\"]
prefixes = []
# What to do when a command touches a path that commonly holds credentials
# (~/.ssh, ~/.aws, ~/.gnupg, .env*, *.pem, *.gpg, *.kdbx, id_rsa, .netrc, ...):
#   \"ask\"   -- always put it in front of you, even if it would otherwise run
#            unattended (default). It forbids nothing; it only refuses to do it
#            SILENTLY.
#   \"deny\"  -- refuse outright.
#   \"allow\" -- treat them like any other path. A WRITE under Claude Code's
#            home still asks: its settings.json runs hooks.
path_policy = \"ask\"
# Additional such paths, beyond the shipped seed. Secrets live wherever you put
# them, so the shipped list protects the common setup and nothing more.
# Each entry is a glob, read as bash reads one (with globstar): * and ? and
# [...] stay inside one directory, ** crosses them, {a,b} is either. A NAME
# (no /) matches anywhere -- \"id_*.pub\", \"*.age\"; a literal name is a file
# or a directory (\".vault\" holds all of it). A PATH is anchored -- \"~/\" at
# a home, \"/\" at the root, otherwise where Shannon started -- and holds
# everything below what it names: \"/srv/keys/*.api\", \"/srv/keys/**\".
# {claude-home} first is Claude Code's home wherever this run found it
# (\"{claude-home}/plugins\"); the seed holds all of it, and ~/.claude too.
# Case never matters.
# Or \"re:\" and a regex, read against the path with . and .. resolved and
# home both ways (~/x and its full spelling); case still does not matter
# unless it says (?-i). Write it in single quotes, so \\ stays \\. A glob in
# a command is checked against every path it could become, so
# re:^/srv/keys/.*pem stops cat /srv/keys/*.
# For example:
#   paths = [\"~/work/vault\", \"*.age\", \"/srv/keys/**\", 're:^/srv/k.*/']
paths = []
# Places carved OUT of those paths, in the same grammar. A pattern holding the
# whole place (~/.claude holds ~/.claude/skills) does not count inside it; one
# naming a file there (*.pem) still does. Carved out is not approved: a read
# outside where Shannon started still asks unless [memory] approves it. These
# are Claude Code's files that hold no credential, which [memory] claude reads:
# under ~/.claude, and under {claude-home} -- Claude Code's home as this run
# found it ($CLAUDE_CONFIG_DIR, else ~/.claude), which leads an entry here or
# in paths. [] makes all of both a credential path again. Your config only.
paths_except = [
    \"~/.claude/projects/*/memory\",
    \"~/.claude/CLAUDE.md\",
    \"~/.claude/skills\",
    \"{claude-home}/projects/*/memory\",
    \"{claude-home}/CLAUDE.md\",
    \"{claude-home}/skills\",
]
# Environment variables holding secrets the agent may USE but never SEE. Each
# becomes an opaque handle -- {{github-token.4f19}} -- that the agent
# writes into a command; the real value is substituted at execution, after you
# have approved the line, and never enters the model's context, the transcript,
# or the log. \"Let the agent use my key\" and \"let the agent read my key\" are
# different requests, and only the second is dangerous. No command is handed
# the variable itself -- Claude Code's included, which otherwise inherit yours.
#
# A command carrying a handle ALWAYS stops for you, even under an \"allow\" rule
# -- otherwise one broad allow becomes a standing permission to spend. A handle
# this session did not mint refuses to run: nothing here could have written it,
# so it is a mistake or an injected instruction. ':secrets' lists what is lent.
# For example: env = [\"GITHUB_TOKEN\", \"DATABASE_URL\"]
env = []
# Alias $HOME to ~ in everything we send and store. ON by default: it makes what
# leaves this machine MORE CORRECT, not merely shorter. An absolute
# /Users/you/... is machine-specific and true nowhere else, while ~ is what a
# portable path looks like to every shell and every model. Your username stops
# travelling, and long paths stop costing tokens, for free. Turn it off if you
# would rather read absolute paths in the transcript.
alias_home = true
# Names that mean \"a credential follows\", beyond the shipped seed (password,
# token, api_key, client_secret, ...). This is the matcher for values with no
# distinguishing SHAPE -- \"password: hunter2seekrit\" is invisible to prefix
# and entropy tests in every format. Matched as a segment SUFFIX of the
# preceding name, so \"stripe_api_key\" counts and \"tokenizer\" does not.
# For example: names = [\"acme-handshake\"]
names = []
# Characters that may separate such a name from its value. Default: = : >
# which covers .env, INI, TOML, CLI flags, YAML, JSON, .pgpass, .npmrc and XML.
# Add \" \" for .netrc-style \"password hunter2\" -- not the default, because
# whitespace makes the prose \"the password is stored in Vault\" mask \"is\".
# Setting this REPLACES the shipped set rather than adding to it, and empty
# means the shipped set -- so write all four:
#   value_delimiters = [\"=\", \":\", \">\", \" \"]
value_delimiters = []

[permissions]
# What you have taught the gate, on top of the shipped rules (:help rules).

# One rule per entry, in the grammar ':analyze' names rules in:
#
#   rules = [
#     \"allow uv run pytest *\",      # a subcommand, with any arguments
#     \"allow find *\",               # a program, with any arguments
#     \"allow find -delete\",         # a flag that deletes, asked no more
#     \"deny rm -r\",                 # a flag, however it is spelled
#     \"ask git push *\",             # ask for a push, beside allow git *
#   ]
#
# A rule says nothing about what it does not name: \"allow git push origin *\"
# also covers \"--force\". A flag is known by every spelling only for a program
# Shannon models; for any other it is matched as written, so \"deny gh api -X\"
# does not catch \"--method\".
#
# The most specific rule decides; where neither lies inside the other, or both
# name the same, the strictest wins -- deny, then ask, then allow -- whatever
# the order (':help rules'). ':rules' lists the policy in force,
# shipped rules included; ':allow', ':deny' and ':ask' add a rule for this
# session only. A rule that cannot be read is named at launch and skipped.
rules = []
# Shipped rules to drop, each written as ':rules' lists it. Only what you name
# goes; a name that matches no shipped rule is reported, not ignored.
#
#   remove = [\"ask :write **/.claude\"]
remove = []
# The scratch directory: the agent's place for temporary files, approved as
# where Shannon started is, and where Claude Code keeps its own. Unset, it is
# /tmp/shannon-<your uid>. Used only if it is a directory, yours, and closed to
# other accounts (Shannon makes it so if it is missing); otherwise launch says
# why, and nothing is approved there.
#
#   scratch = \"/tmp/shannon-me\"

[claude_code]
# Claude Code as a provider (a `claude-code/...` model). Shannon's gate decides
# every tool Claude Code calls: its shell commands, reads and writes by the
# rules above; a tool the rules cannot read -- a subagent, an MCP server, the
# web -- asks every time.
#
# ONE DIFFERENCE FROM SHANNON'S OWN TURNS: Claude Code runs the tool, and its
# model sees the output AS IS. Shannon redacts what it shows you and what it
# stores, but nothing stands between a tool's output and Claude Code's model --
# the gate deciding whether a call runs is the only layer.

# The web tools (WebFetch, WebSearch): \"ask\" (the default) or \"allow\".
# Allowing them lets the model fetch any URL unasked -- and a URL can carry
# whatever the model has read (`https://example.com/?q=<your file>`). Only
# your own config may set it.
web = \"ask\"
# A path that commonly holds credentials ([secrets] paths), on a call Claude
# Code runs: \"deny\" (the default), \"ask\", or \"allow\" -- used in place of
# [secrets] path_policy for these calls, because of the difference above.
# Only your own config may set it.
path_policy = \"deny\"
# Whether Claude Code compacts its own context near its window's end (its
# DISABLE_AUTO_COMPACT). Off, the window fills until a request is refused and
# ':compact' is yours to run. Either way, a compaction Claude Code makes is
# said: its model then works from its summary, while Shannon keeps every turn.
auto_compact = true
# Whether Claude Code may run work in the background: agents asked to run in
# the background, and Bash's run_in_background (its
# CLAUDE_CODE_DISABLE_BACKGROUND_TASKS). A background agent's calls never reach
# Claude Code's output, so Shannon cannot show them to you, and refuses every
# one. Off, that work runs in the foreground, each call shown and decided here.
background_tasks = true
# What Claude Code's process -- and so every command it runs -- is given of
# your environment: \"inherit\" (the default), all of it, as Claude Code run by
# hand has, less Shannon's key, what [secrets] env lends and [exec] withhold;
# or \"minimal\", only what Shannon's own commands get ([exec] env included)
# and Claude Code's own config directory, so a credential Shannon does not
# hold is not there to print. Only your own config may set it.
env = \"inherit\"

[memory]
# What the model is told beyond the conversation, recorded in the session as it
# is sent (':memories' lists it, ':help memory' says how).
#
# A LOCATION is one place when it begins with ~, /, {config} (Shannon's config
# directory) or {start} (where Shannon started), and otherwise is looked for in
# where Shannon started and each directory above it up to the project's root
# (`project_root` below).
# {project} (this project's name) is filled in wherever it stands. It may be a
# file or a directory. The lists below STACK: your config's list replaces this
# one, and a project's config adds to it.

# Whether the model writes memories: \"auto\" (writes to `write` run unasked),
# \"ask\" (they ask), or \"off\" (it is not told how). `:set memory=` sets it.
mode = \"auto\"
# Instruction files, read whole into every session; a directory is every *.md
# in it.
instructions = [\"AGENTS.md\", \"SHANNON.md\"]
# Claude Code's own files, CLAUDE.md and .claude/skills: \"auto\" (a Claude Code
# session reads them, as claude does; a session through the API does not),
# \"on\" (the API sessions read them too), or \"off\" (neither).
claude = \"auto\"
# Where memories are READ: a directory (its MEMORY.md is read, its other files
# listed) or a file (read whole).
paths = [\"{config}/memory\", \"{config}/projects/{project}/memory\"]
# Where the model WRITES them -- a directory, made if missing, yours and closed
# to other accounts. Only your own config may set it.
write = \"{config}/projects/{project}/memory\"
# How {project} is named: \"hash\" (the directory Shannon started in, its name
# and a short hash of its path: my-project-3f9a2c71) or \"claude-code\" (as
# Claude Code names it, from the repository's root: -Users-me-my-project, so a
# subdirectory and every worktree are one project). Changing it moves where
# memories are kept; only your own config may set it.
project = \"hash\"
# What marks the project's root: the nearest directory above where Shannon
# started that holds one of these names is the root, where a location's walk
# stops (gutentags' markers). [] (or none found): where Shannon started alone.
# It decides what a location may read, so only your own config may set it.
project_root = [\".git\", \".hg\", \".svn\", \".bzr\", \"_darcs\", \"_FOSSIL_\", \".fslckout\"]
# What is NOT read: an instruction file or a memory, or a directory and all in
# it. Not told, not listed, and a read of it refused. What the model already
# has stays. `x` in `:memories` sets it for the session; this list STACKS.
disabled = []

[skills]
# Skills: instructions the model loads by name (:help skills).

# Where skills are found: a directory (each <name>/SKILL.md in it) or a file
# (one skill). Locations as [memory]'s; this list STACKS too. `:skills` lists
# them, `:skill <name>` runs one, and the model loads one by name.
paths = [\"{config}/skills\", \".shannon/skills\"]
# Skills NOT used: a name (every skill of it), or a location and each skill in
# it. Not listed, not loaded (by Shannon or by Claude Code's Skill tool), and a
# read of one refused. `x` in `:skills` sets it for the session; this STACKS.
disabled = []

[mcp]
# MCP servers -- today Claude Code's, which its own settings configure
# (`claude mcp`).

# Servers NOT used, by name. Every call to one of a disabled server's tools is
# refused; it still starts, and its tools are still listed. `x` on the MCP list
# (`:skills`, then Tab) sets it for the session; this STACKS.
disabled = []

# The substitution table: a local value that travels as a short name.
# Outbound, the value becomes the name; inbound, the name becomes the value.
# One mechanism, three uses -- and only the third needs a security argument:
#
#   privacy       your username and paths are nobody else's business
#   token economy a 60-char path repeated 400 times in a build log is
#                 expensive; {{a:repo}} is not
#   credentials   a value the agent may SPEND but never SEE
#
# SENSITIVE is inferred from the source and overridable. A value written here
# in cleartext has already been decided not to be a secret (nobody protecting a
# real credential can put it in a config file), so it defaults to false; an env
# var named here usually IS one, so it defaults to true.
#
# Only sensitive entries gate. A secret faces you on every command that spends
# it, defeats an \"allow\" rule, and carries an unguessable session nonce. A
# plain alias resolves silently at the gate -- it grants no authority the
# machine did not already have, and gating it would train you to dismiss
# prompts. ':alias' lists them all with their values, on your screen only --
# the model is told the handles.
#
# [[alias]]
# name = \"repo\"
# value = \"/Users/you/src/some-project\"    # -> {{a:repo}}
#
# [[alias]]
# name = \"github-token\"
# env = \"GITHUB_TOKEN\"                             # -> {{github-token.<nonce>}}

# Keymap overrides (optional) -- remap any key in any context. The defaults are
# canonical vim and live in code; a [keymap.<context>] table is a sparse diff
# over them. Contexts: motion, goto, normal, visual, insert, window, z,
# text-object.
# Chords: a bare char (\"j\"), or bracketed -- <C-w> (ctrl), <S-l> (shift),
# <Esc>/<Enter>/<Space>/<Tab>. Actions are stable names -- run ':keymap' for
# every one of them beside the key it is on today, or ':keymap <context>' for
# one context. Defaults worth knowing: in Normal, Enter and <C-Enter> both
# 'submit'; in Insert, <C-Enter> is the direct-submit key (plain Enter is a
# newline).
# Examples (commented out -- uncomment to apply):
#
# [keymap.insert]
# \"<C-d>\" = \"submit\"          # add Ctrl-D as a second direct-submit key
# [keymap.motion]
# \"j\" = \"word-forward\"        # make j jump words (silly, but it works)
";

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    /// **Premise for the scrub below: a parse error quotes the source line.**
    ///
    /// The fix at the `warn!` site is only worth anything if the error text
    /// really does carry the offending line — `[[alias]] value = "…"` means
    /// that line can be a literal secret. Asserting the premise separately so a
    /// future `toml` that stops quoting turns this into a visible failure
    /// rather than silently making the scrub a no-op
    /// (`[[feedback_assert_the_subject]]`).
    #[test]
    fn a_parse_error_quotes_the_offending_line() {
        let planted = "correcthorsebatterystaple";
        // The error must be ON the secret's own line: `toml` quotes the line it
        // points at and no other, so a typo one line *below* a secret does not
        // leak it. A stray token after the value is the shape that does — and
        // is exactly what a hand-edited config produces.
        let text = format!("[[alias]]\nname = \"tok\"\nvalue = \"{planted}\" oops\n");
        let err = Config::parse(&text).expect_err("a stray token is a parse error");
        assert!(
            err.to_string().contains(planted),
            "premise: the error renders the source line, so it can leak a value\n{err}"
        );
        // And the half that decides the FIX: redaction alone does not save it.
        // An alias value need not look like a credential — this one is under
        // `MIN_ENTROPY_LEN` and matches no issuer prefix — so dropping the
        // snippet has to be structural rather than a scrub.
        let scrubbed = crate::secrets::redact(
            &err.to_string(),
            &crate::secrets::SecretsConfig::default(),
            &crate::Aliases::default(),
        )
        .text;
        assert!(
            scrubbed.contains(planted),
            "premise: scrubbing cannot reach a value that looks like ordinary text"
        );
    }

    /// **The logged form of a parse failure carries no source text.**
    ///
    /// The pair to the premise test above: that one proves the rendered error
    /// leaks, this one proves the thing we actually write does not — and names
    /// the line so the message stays diagnosable.
    ///
    /// **The planted value is deliberately one the scrubber cannot catch.** A
    /// `sk-ant-…` here passed with `message()` mutated to the full `Display`,
    /// because redaction caught it by issuer prefix either way — the assertion
    /// was satisfied by whichever defence happened to fire, so it could not
    /// tell them apart (`[[feedback_assert_the_subject]]`). An all-lowercase
    /// passphrase is under `MIN_ENTROPY_LEN` and has no prefix, so it is
    /// invisible to redaction and only the dropped snippet can save it — which
    /// is the exact claim `log_safe_parse_error` makes for `message()`.
    #[test]
    fn a_logged_parse_error_drops_the_source_line() {
        let planted = "correcthorsebatterystaple";
        let text = format!("[[alias]]\nname = \"tok\"\nvalue = \"{planted}\" oops\n");
        let err = Config::parse(&text).expect_err("a stray token is a parse error");
        let (message, line) = Config::log_safe_parse_error(&err, &text);
        assert!(
            !message.contains(planted),
            "the logged message still carries the value: {message}"
        );
        // Not merely absent — absent because the snippet is gone, while the
        // message that makes the line worth logging survives.
        assert!(
            !message.is_empty(),
            "a message with nothing in it is no log"
        );
        assert_eq!(line, 3, "the line number is what replaces the quoted text");
    }

    /// **And what launch SHOWS carries no source text either** — because what
    /// the user is shown is logged too (the TUI's notice door), so a quote on
    /// screen was a quote on disk, whatever the `warn!` beside it did. It said
    /// "config NOT LOADED — {err}", the rendered error with the line in it.
    #[test]
    fn the_launch_notice_for_a_bad_file_names_the_line_not_its_text() {
        let planted = "correcthorsebatterystaple";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            format!("[[alias]]\nname = \"tok\"\nvalue = \"{planted}\" oops\n"),
        )
        .unwrap();
        let (_, said) = Config::load_path_reporting(&path);
        let note = said.join("\n");
        assert!(note.contains("NOT LOADED"), "premise: refused\n{note}");
        assert!(
            !note.contains(planted),
            "the value reached the notice:\n{note}"
        );
        assert!(note.contains("line 3"), "and says where:\n{note}");
    }

    /// The scrub is the *second* defence, and it has to be wired up too.
    ///
    /// `message()` dropping the snippet is what catches a low-entropy secret;
    /// this covers the other direction — serde quoting a rejected value inside
    /// the message itself, where only redaction can reach it.
    #[test]
    fn a_value_quoted_inside_the_message_is_scrubbed() {
        let planted = "sk-ant-api03-PLANTEDVALUE0123456789";
        // `height` wants an integer, so serde rejects the string *by value*.
        let text = format!("[tui.prompt]\nheight = \"{planted}\"\n");
        let err = Config::parse(&text).expect_err("a string is not a height");
        assert!(
            err.message().contains(planted),
            "premise: serde quotes the value it rejected — {}",
            err.message()
        );
        let (message, _) = Config::log_safe_parse_error(&err, &text);
        assert!(
            !message.contains(planted),
            "a value quoted inside the message reached the log: {message}"
        );
    }

    #[test]
    fn empty_document_is_all_defaults() {
        let config = Config::parse("").expect("empty config parses");
        assert_eq!(config, Config::default());
        assert_eq!(config.tui.prompt.height, DEFAULT_PROMPT_HEIGHT);
    }

    #[test]
    fn sparse_override_changes_only_named_key() {
        let config = Config::parse("[tui.prompt]\nheight = 4\n").expect("parses");
        // The one named key changed; everything else stays default. With a
        // one-field config that's tautological today, but the assertion is
        // the contract that protects it as the struct grows.
        assert_eq!(config.tui.prompt.height, 4);
        let mut expected = Config::default();
        expected.tui.prompt.height = 4;
        assert_eq!(config, expected);
    }

    #[test]
    fn every_tui_knob_overrides_sparsely_from_its_documented_default() {
        // One invariant, all the simple knobs (consolidated in the M3 test
        // review from seven per-knob tests): each is a sparse-overridable
        // diff over the coded default. The DEFAULT_TOML completeness test
        // guards the defaults themselves; this guards the wire names.
        let d = Config::default();
        assert_eq!(d.tui.prompt.start_mode, StartMode::Normal);
        assert_eq!(d.tui.resume, ResumeMode::Fresh);
        assert_eq!(d.tui.graph_sort, GraphSort::Recency);
        assert_eq!(d.tui.graph_style, GraphStyle::Git);
        assert_eq!(d.tui.graph_chars, GraphChars::Unicode);
        assert_eq!(d.tui.graph_ids, GraphIds::After);
        assert_eq!(d.tui.scrolloff, DEFAULT_SCROLLOFF);
        assert!(d.tui.confirm_delete);

        let c = Config::parse(
            "[tui]\n\
             resume = \"last\"\n\
             graph_sort = \"hash\"\n\
             graph_style = \"tree\"\n\
             graph_chars = \"ascii\"\n\
             graph_ids = \"hidden\"\n\
             scrolloff = 0\n\
             confirm_delete = false\n\
             [tui.prompt]\n\
             start_mode = \"insert\"\n",
        )
        .expect("parses");
        assert_eq!(c.tui.prompt.start_mode, StartMode::Insert);
        assert_eq!(c.tui.resume, ResumeMode::Last);
        assert_eq!(c.tui.graph_sort, GraphSort::Hash);
        assert_eq!(c.tui.graph_style, GraphStyle::Tree);
        assert_eq!(c.tui.graph_chars, GraphChars::Ascii);
        assert_eq!(c.tui.graph_ids, GraphIds::Hidden);
        assert_eq!(c.tui.scrolloff, 0, "purist zero is honored");
        assert!(!c.tui.confirm_delete);
        // Sparse: the un-named keys kept their defaults.
        assert_eq!(c.tui.prompt.height, DEFAULT_PROMPT_HEIGHT);
        assert_eq!(
            c.tui.mouse, d.tui.mouse,
            "an un-named key keeps the coded default, whatever it is"
        );
        // …and NAMING it still overrides. Asserting only the sparse half made
        // this a test of one constant rather than of the mechanism: flipping
        // the default was enough to fail it, and no override was ever exercised
        // ([[feedback_fixture_omits_the_axis]]).
        let named = Config::parse("[tui]\nmouse = true\n").expect("parses");
        assert!(named.tui.mouse, "naming a key overrides the default");
        let named = Config::parse("[tui]\nmouse = false\n").expect("parses");
        assert!(!named.tui.mouse, "…in both directions");
    }

    #[test]
    fn squash_instruction_parses_and_defaults() {
        assert_eq!(
            Config::default().merge.squash_instruction,
            DEFAULT_SQUASH_INSTRUCTION
        );
        let c =
            Config::parse("[merge]\nsquash_instruction = \"just the facts\"\n").expect("parses");
        assert_eq!(c.merge.squash_instruction, "just the facts");
        // `[merge]` present but empty still yields the real default (field-level
        // serde default), not an empty string.
        let c = Config::parse("[merge]\n").expect("parses");
        assert_eq!(c.merge.squash_instruction, DEFAULT_SQUASH_INSTRUCTION);
    }

    #[test]
    fn continue_config_parses_and_defaults() {
        let d = Config::default().continue_;
        assert_eq!(d.strategy, ContinueStrategy::Instruct, "universal default");
        assert!(
            d.instruction.contains("{tail}"),
            "the template carries the splice point"
        );
        // The section is spelled `continue` in TOML (the keyword problem is
        // Rust's, not the user's), and it's a sparse diff like everything else.
        let c = Config::parse("[continue]\nstrategy = \"prefill\"\n").expect("parses");
        assert_eq!(c.continue_.strategy, ContinueStrategy::Prefill);
        assert_eq!(c.continue_.instruction, DEFAULT_CONTINUE_INSTRUCTION);
    }

    #[test]
    fn agents_guardrails_parse_and_default() {
        let d = Config::default().agents;
        assert_eq!(
            (d.max_depth, d.max_fanout, d.on_limit),
            (DEFAULT_MAX_DEPTH, DEFAULT_MAX_FANOUT, OnLimit::Confirm)
        );
        // Sparse: naming one key keeps field-level defaults for the rest.
        let c = Config::parse("[agents]\nmax_depth = 1\n").expect("parses");
        assert_eq!(c.agents.max_depth, 1);
        assert_eq!(c.agents.max_fanout, DEFAULT_MAX_FANOUT);
        // Every posture parses — the limit behavior is a setting, not policy.
        for (text, want) in [
            ("confirm", OnLimit::Confirm),
            ("block", OnLimit::Block),
            ("advisory", OnLimit::Advisory),
        ] {
            let c = Config::parse(&format!("[agents]\non_limit = \"{text}\"\n")).expect("parses");
            assert_eq!(c.agents.on_limit, want, "{text}");
        }
        // A typo'd key is rejected (nested deny_unknown_fields), not ignored.
        assert!(Config::parse("[agents]\nmax_dept = 2\n").is_err());
    }

    #[test]
    fn graph_glyphs_parse_as_a_sparse_map_and_default_empty() {
        assert!(Config::default().tui.graph_glyphs.is_empty());
        let c = Config::parse("[tui.graph_glyphs]\nchild = \"◇\"\n").expect("parses");
        assert_eq!(
            c.tui.graph_glyphs.get("child").map(String::as_str),
            Some("◇")
        );
        // Unknown ROLES are accepted at parse time (it's an open map, like
        // keymap) — the TUI warns and ignores when resolving.
        assert!(Config::parse("[tui.graph_glyphs]\nnope = \"x\"\n").is_ok());
    }

    #[test]
    fn parse_ready_parses_and_clamps() {
        // Like `parse`, but also clamps out-of-range knobs (what `:config edit`
        // applies live) — and SAYS so, which it used to drop on the floor.
        let (c, notes) = Config::parse_ready("[tui.prompt]\nheight = 65535\n").expect("parses");
        assert_eq!(c.tui.prompt.height, MAX_PROMPT_HEIGHT);
        assert!(
            notes
                .iter()
                .any(|n| n.contains("tui.prompt.height 65535 is out of range")),
            "the clamp is reported: {notes:?}"
        );
    }

    #[test]
    fn a_written_config_file_loads_and_overrides_defaults() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        // `git` is the coded default; a file that sets `tree` must win.
        std::fs::write(&path, "[tui]\ngraph_style = \"tree\"\n").unwrap();
        assert_eq!(Config::load_path(&path).tui.graph_style, GraphStyle::Tree);
    }

    /// **An alert's `notify` is a switch or words, and says what it can be**
    /// — the sentence its reader gives, where serde's own for an untagged
    /// enum named no spelling; and the switches over every alert's bell and
    /// notification start on.
    #[test]
    fn an_alerts_notify_is_a_switch_or_words() {
        let read = |value: &str| {
            Config::parse(&format!(
                "[tui.alert.x]\nwhen = \"queued\"\nnotify = {value}\n"
            ))
            .map(|c| c.tui.alert["x"].notify.clone())
        };
        assert_eq!(read("true").unwrap(), Some(Notify::On(true)));
        assert_eq!(read("false").unwrap(), Some(Notify::On(false)));
        // Quoted, as `:set` writes a value over words: still the switch.
        assert_eq!(read("\"false\"").unwrap(), Some(Notify::On(false)));
        assert_eq!(read("\"true\"").unwrap(), Some(Notify::On(true)));
        assert_eq!(
            read("\"$session needs you\"").unwrap(),
            Some(Notify::Says("$session needs you".into()))
        );
        let why = read("3").unwrap_err().to_string();
        assert!(why.contains("true, false, or a message"), "{why}");
        let alerts = AlertsConfig::default();
        assert!(
            alerts.bell && alerts.notify,
            "on by default (Steven, 10-23)"
        );
        assert_eq!(alerts.notify_via, NotifyVia::Osc9);
        let via = Config::parse("[tui.alerts]\nnotify_via = \"tmux\"\n").unwrap();
        assert_eq!(via.tui.alerts.notify_via, NotifyVia::Tmux);
    }

    #[test]
    fn writing_the_default_template_round_trips_through_load() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, DEFAULT_TOML).unwrap();
        assert_eq!(Config::load_path(&path), Config::default());
    }

    #[test]
    fn load_path_degrades_to_defaults_on_missing_or_broken() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(
            Config::load_path(&dir.path().join("nope.toml")),
            Config::default(),
            "missing file → defaults"
        );
        let bad = dir.path().join("bad.toml");
        std::fs::write(&bad, "[tui.prompt]\nheight = \"tall\"\n").unwrap(); // wrong type
        assert_eq!(
            Config::load_path(&bad),
            Config::default(),
            "broken file → defaults, no panic"
        );
    }

    #[test]
    fn present_table_missing_field_still_defaults() {
        // `[tui.prompt]` exists but omits `height`: must be the real
        // default (10), not u16's 0 — the field-level `serde(default)`.
        let config = Config::parse("[tui.prompt]\n").expect("parses");
        assert_eq!(config.tui.prompt.height, DEFAULT_PROMPT_HEIGHT);
    }

    #[test]
    fn unknown_top_level_key_is_captured_not_rejected() {
        // Forward-compat: an unknown top-level section parses fine and is
        // captured for a warning rather than erroring.
        let config = Config::parse("[future_feature]\nx = 1\n").expect("unknown top-level is ok");
        assert!(config.unknown.contains_key("future_feature"));
        assert_eq!(config.tui.prompt.height, DEFAULT_PROMPT_HEIGHT);
    }

    #[test]
    fn unknown_nested_key_is_rejected() {
        // Anti-papercut: a typo inside a known section is surfaced as an
        // error, not silently dropped.
        let err = Config::parse("[tui.prompt]\nheigth = 12\n");
        assert!(err.is_err(), "misspelled nested key must be rejected");
    }

    #[test]
    fn default_toml_round_trips_to_default() {
        let parsed = Config::parse(DEFAULT_TOML).expect("DEFAULT_TOML parses");
        assert_eq!(
            parsed,
            Config::default(),
            "DEFAULT_TOML must equal the coded defaults",
        );
    }

    #[test]
    fn default_toml_is_complete() {
        // Stronger than round-trip: serialize the coded defaults and
        // compare *tables*, so a newly-added field that DEFAULT_TOML
        // forgets fails here (the round-trip alone would still pass,
        // because the new field would just default on parse).
        let from_defaults = Config::default().as_table();
        let from_doc: toml::Table = DEFAULT_TOML.parse().expect("DEFAULT_TOML is a table");
        assert_eq!(
            toml::Value::Table(from_doc),
            toml::Value::Table(from_defaults),
            "DEFAULT_TOML is missing or misstating a field",
        );
    }

    #[test]
    fn resolve_path_prefers_xdg() {
        let path = resolve_config_path(Some(OsStr::new("/xdg")), Some(OsStr::new("/home/u")))
            .expect("path resolves");
        assert_eq!(path, PathBuf::from("/xdg/shannon/config.toml"));
    }

    #[test]
    fn resolve_path_falls_back_to_home_dotconfig() {
        let path = resolve_config_path(None, Some(OsStr::new("/home/u"))).expect("path resolves");
        assert_eq!(path, PathBuf::from("/home/u/.config/shannon/config.toml"));
    }

    #[test]
    fn resolve_path_none_when_neither_set() {
        assert_eq!(resolve_config_path(None, None), None);
    }

    #[test]
    fn resolve_path_ignores_empty_or_relative_xdg() {
        // Empty / relative XDG_CONFIG_HOME is "invalid" per the spec: fall
        // back to $HOME/.config rather than read a stray relative path.
        let empty = OsString::new();
        assert_eq!(
            resolve_config_path(Some(&empty), Some(OsStr::new("/home/u"))),
            Some(PathBuf::from("/home/u/.config/shannon/config.toml")),
        );
        assert_eq!(
            resolve_config_path(Some(OsStr::new("relative/dir")), None),
            None
        );
    }

    /// **Nor an empty, relative or root `HOME`** (review 2026-10-11, E2): it
    /// put the config, the user's program models and the store under the
    /// launch directory — the project.
    #[test]
    fn resolve_path_ignores_an_empty_or_relative_home() {
        for home in ["", "home", "./h", "/"] {
            assert_eq!(
                resolve_config_path(None, Some(OsStr::new(home))),
                None,
                "{home:?}"
            );
            assert_eq!(
                resolve_state_dir(None, Some(OsStr::new(home))),
                None,
                "{home:?}"
            );
        }
    }

    #[test]
    fn resolve_state_dir_prefers_xdg_then_home() {
        assert_eq!(
            resolve_state_dir(Some(OsStr::new("/xdg/state")), Some(OsStr::new("/home/u"))),
            Some(PathBuf::from("/xdg/state/shannon")),
        );
        assert_eq!(
            resolve_state_dir(None, Some(OsStr::new("/home/u"))),
            Some(PathBuf::from("/home/u/.local/state/shannon")),
        );
        assert_eq!(resolve_state_dir(None, None), None);
        // Empty/relative XDG_STATE_HOME is invalid → fall back to $HOME.
        assert_eq!(
            resolve_state_dir(Some(OsStr::new("relative")), Some(OsStr::new("/home/u"))),
            Some(PathBuf::from("/home/u/.local/state/shannon")),
        );
    }

    /// The system file is the FIRST that exists under `$XDG_CONFIG_DIRS`
    /// (Neovim's `sysinit.vim` rule), `/etc/xdg` by default, relative entries
    /// skipped.
    #[test]
    fn the_system_config_is_the_first_that_exists_in_xdg_config_dirs() {
        let only = |hit: &'static str| move |p: &Path| p == Path::new(hit);
        assert_eq!(
            resolve_system_config_path(None, only("/etc/xdg/shannon/config.toml")),
            Some(PathBuf::from("/etc/xdg/shannon/config.toml")),
            "unset: the spec's default"
        );
        assert_eq!(
            resolve_system_config_path(Some(OsStr::new("")), only("/etc/xdg/shannon/config.toml")),
            Some(PathBuf::from("/etc/xdg/shannon/config.toml")),
            "empty counts as unset"
        );
        // Everything "exists", so only the skip keeps `rel` out, and only order
        // picks `/a` over `/b`.
        assert_eq!(
            resolve_system_config_path(Some(OsStr::new("rel:/a:/b")), |_| true),
            Some(PathBuf::from("/a/shannon/config.toml")),
            "the first that exists wins; a relative entry is skipped"
        );
        assert_eq!(
            resolve_system_config_path(Some(OsStr::new("/a:/b")), |_| false),
            None,
            "none installed: the normal case"
        );
    }

    /// What `:source` reports it changed: dotted keys, at depth, only what
    /// differs — and a key present on one side only counts.
    #[test]
    fn changed_keys_are_dotted_and_only_what_differs() {
        let before = Config::default();
        let mut after = Config::default();
        after.tui.scrolloff = 1;
        after.tui.prompt.height = 3;
        after
            .keymap
            .entry("normal".into())
            .or_default()
            .insert("Q".into(), "enter-command".into());
        assert_eq!(
            before.changed_keys(&after),
            ["keymap", "tui.prompt.height", "tui.scrolloff"]
        );
        assert!(before.changed_keys(&before).is_empty());
    }

    /// A key is under a prefix at a DOT, not wherever the text happens to
    /// continue — `secretsx` is not a safety setting because it starts with
    /// `secrets`.
    #[test]
    fn a_key_is_under_a_prefix_only_at_a_dot() {
        assert!(key_is_under("tui.mouse", "tui"));
        assert!(key_is_under("tui.mouse", "tui.mouse"));
        assert!(!key_is_under("tui.mouse", "tui.mou"));
        assert!(!key_is_under("secretsx", "secrets"));
        assert!(!key_is_under(
            "tui.confirm_delete_all",
            "tui.confirm_delete"
        ));
        assert!(
            !key_is_under("tui", "tui.mouse"),
            "a section is not under its key"
        );
    }

    /// **serde's quote of the rejected value goes; the shape stays** —
    /// including an escaped quote inside the value, and several operands.
    #[test]
    fn a_rejected_value_is_not_quoted() {
        assert_eq!(
            without_operands("invalid type: string \"my-db-pass\", expected a sequence"),
            "invalid type: string, expected a sequence"
        );
        assert_eq!(
            without_operands("invalid value: string \"a\\\"b\", expected x"),
            "invalid value: string, expected x"
        );
        assert_eq!(
            without_operands("unknown variant `h4nter`, expected `fresh` or `last`"),
            "unknown variant, expected `fresh` or `last`"
        );
        let untouched = "unknown field `scrolof`, expected one of `prompt`, `mouse`";
        assert_eq!(without_operands(untouched), untouched, "key names stay");
        assert_eq!(without_operands("string \"é"), "string", "unterminated");
    }

    /// **The line an error names is the line it is on** — a typo'd key at
    /// the start of line 2 is line 2 (Reviewers B and C: it said 1).
    #[test]
    fn a_parse_error_names_its_own_line() {
        for (text, line) in [
            ("[tui]\nscrolof = 9\n", 2),
            ("[tui\n", 1),
            ("[tui]\n\n\n[tui.prompt]\nheigth = 3\n", 5),
            ("[tui]\nscrolloff = \"x\"\n", 2),
        ] {
            let err = Config::parse(text).expect_err("does not fit");
            let said = Config::describe_parse_error(&err, text);
            assert!(
                said.starts_with(&format!("line {line}: ")),
                "{text:?}: {said}"
            );
        }
        // The natural mistake with a secret: a string where a list belongs.
        let text = "[secrets]\nextra = \"c4n4ry7\"\n";
        let err = Config::parse(text).expect_err("a string is not a list");
        let said = Config::describe_parse_error(&err, text);
        assert!(said.starts_with("line 2: "), "{said}");
        assert!(!said.contains("c4n4ry7"), "the value is not quoted: {said}");
    }

    /// **A regex rule in TOML wants a literal string** (`'…'`), as the manual
    /// says: in a basic one (`"…"`), `\s` is an escape TOML does not have, so
    /// the file is refused — never read as some other pattern.
    #[test]
    fn a_regex_rule_in_toml_wants_a_literal_string() {
        let basic = r#"[permissions]
rules = ["deny re:^rm(\s|$)"]
"#;
        assert!(Config::parse(basic).is_err(), "`\\s` is not a TOML escape");
        let literal = r#"[permissions]
rules = ['deny re:^rm(\s|$)']
"#;
        let config = Config::parse(literal).expect("a literal string keeps `\\s`");
        assert_eq!(config.permissions.rules, [r"deny re:^rm(\s|$)"]);
    }

    /// **A rule list is laid out for reading** (Steven, 2026-10-03:
    /// readability of rules is a safety feature). One entry a line, a
    /// trailing comma or not, comments between entries — all TOML, all
    /// read. Unquoted entries are not TOML, and the refusal says what is.
    #[test]
    fn a_rule_list_may_span_lines() {
        for text in [
            "[permissions]\nrules = [\n   \"allow ls\",\n   \"ask find:-exec\",\n]\n",
            "[permissions]\nrules = [\"allow ls\",\n         \"ask find:-exec\",\n]\n",
            "[permissions]\nrules = [\n   \"allow ls\",\n   \"ask find:-exec\"\n]\n",
            "[permissions]\nrules = [\n  \"allow ls\",  # why\n  # a line of its own\n  \"ask find:-exec\",\n]\n",
        ] {
            let config = Config::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(config.permissions.rules, ["allow ls", "ask find:-exec"]);
        }
        let bare = "[permissions]\nrules = [allow ls, ask find:-exec]\n";
        let err = Config::parse(bare).expect_err("an unquoted rule is not TOML");
        let said = Config::describe_parse_error(&err, bare);
        assert!(said.contains("each entry quoted"), "{said}");
    }

    /// **One long regex, across lines** — a TOML multi-line literal string
    /// (`'''`) and the regex's own verbose flag `(?x)`, where whitespace is
    /// not matched and `#` comments to the end of its line. The pattern the
    /// manual shows, written to be read, decides as the one-line form does.
    #[test]
    fn a_long_regex_rule_may_span_lines() {
        use crate::permission::Decision;
        let text = r#"[permissions]
rules = [
  '''deny re:(?x)
     ^(?:.*/|\\)?rm                   # the program, bare or pathed
     (?:\s+--?[A-Za-z0-9][\w-]*)*     # then its flags
     (?:\s|$)                         # then a space, or the end
  ''',
]
"#;
        let config = Config::parse(text).expect("a literal string");
        let (rules, problems) =
            crate::permission::RuleSet::configured(&config.permissions, &[], &[]);
        assert!(problems.is_empty(), "{problems:?}");
        for (line, want) in [
            ("rm -rf x", Decision::Deny),
            ("/bin/rm -r x", Decision::Deny),
            ("cd x && rm -rf y", Decision::Deny),
            ("rmdir x", Decision::Ask),
            ("echo rm", Decision::Allow),
        ] {
            assert_eq!(rules.decide_line(line).decision, want, "{line}");
        }

        // What cannot be honoured is said on one line, at load, as a notice:
        // a rule that does not read, a pattern that does not compile, a
        // `remove` that names nothing shipped.
        let permissions = crate::config::PermissionsConfig {
            rules: vec!["allwo\n  rm *".into(), "deny re:(?x)\n  (unclosed\n".into()],
            remove: vec!["ask\n  frobnicate:-x".into(), "allwo\n  x".into()],
            ..Default::default()
        };
        let (_, problems) = crate::permission::RuleSet::configured(&permissions, &[], &[]);
        assert_eq!(problems.len(), 4, "{problems:?}");
        for said in &problems {
            assert!(!said.contains('\n'), "{said:?}");
        }
    }

    /// **A `[secrets] paths` regex that does not compile is said at load**:
    /// it protects nothing, and an entry that silently matched nothing would
    /// read as protection.
    #[test]
    fn a_broken_secrets_regex_is_named_at_load() {
        let text = "[secrets]\npaths = ['re:^/srv/keys/', 're:(unclosed']\n";
        let (_, notes) = Config::parse_ready(text).expect("the TOML is fine");
        let said: Vec<_> = notes
            .iter()
            .filter(|n| n.contains("secrets.paths"))
            .collect();
        assert!(
            said.len() == 1
                && said[0].contains("(unclosed")
                && said[0].contains("protects nothing"),
            "{notes:?}"
        );
    }

    /// **A key written twice says which form IS accepted** (Steven,
    /// 2026-10-03: rejecting duplicates is fine, "just we should specify which
    /// is accepted") — the one list, which may span lines, at the line of the
    /// second.
    #[test]
    fn a_duplicate_key_names_the_accepted_form() {
        let text = "[permissions]\nrules = [\"deny rm:-r\"]\nrules = [\"allow ls *\"]\n";
        let err = Config::parse(text).expect_err("a key twice in one table");
        let said = Config::describe_parse_error(&err, text);
        assert!(said.starts_with("line 3: "), "the second one: {said}");
        assert!(said.contains("once"), "{said}");
        assert!(said.contains("may span lines"), "{said}");
    }

    /// **A section this build does not know is captured, never shown**: it
    /// is not in the serialized config (what `:set`, `:set all` and
    /// `changed_keys` read), though `review` still names it.
    #[test]
    fn an_unknown_section_is_not_serialized() {
        // `[plugins]`, not `[mcp]`: that one is known now (`McpConfig`).
        let mut c = Config::parse("[plugins.gh]\ntoken = \"t0k3n5x\"\n").unwrap();
        let table = c.as_table();
        assert!(!table.is_empty(), "premise: it serializes");
        assert!(!table.contains_key("plugins"), "{table:?}");
        assert!(Config::default().changed_keys(&c).is_empty());
        let notes = c.review();
        assert!(notes.iter().any(|n| n.contains("plugins")), "{notes:?}");
        assert!(!notes.iter().any(|n| n.contains("t0k3n5x")), "{notes:?}");
    }

    /// **A rule the gate cannot honour is named where the gate is built**
    /// (the app's `rebuild_rules`, from `RuleSet::configured_by`): a typo'd
    /// rule, and a `remove` that names nothing shipped. Not by the config's
    /// own review, which knows no user's models and so judged rules by the
    /// shipped set (review 2026-10-11, A and C).
    #[test]
    fn a_permissions_entry_that_cannot_apply_is_named_at_load() {
        let mut c = Config::parse(
            "[permissions]\nrules = [\"allwo rm *\", \"allow ls *\"]\nremove = [\"deny ls:-l\"]\n",
        )
        .unwrap();
        assert!(c.review().is_empty(), "the review does not judge rules");
        let (_, problems) =
            crate::permission::RuleSet::configured_by(&c.permissions, &[], &[], None);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(
            problems.iter().any(|n| n.contains("allwo rm *")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|n| n.contains("not a shipped rule")),
            "{problems:?}"
        );
    }

    /// **Every safety prefix names a setting that exists** — a typo'd entry
    /// (`secret`) would never match anything, and nothing else would notice.
    /// (Whether the list is COMPLETE is a judgement, not a test: see the doc.)
    #[test]
    fn every_safety_prefix_names_a_real_setting() {
        fn dotted(table: &toml::Table, prefix: &str, out: &mut Vec<String>) {
            for (key, value) in table {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                if let toml::Value::Table(inner) = value {
                    dotted(inner, &path, out);
                }
                out.push(path);
            }
        }
        // `[[alias]]` is left out of the defaults when empty, so give it one.
        let mut config = Config::default();
        config.alias.push(AliasSpec {
            name: "a".into(),
            value: Some("b".into()),
            env: None,
            sensitive: None,
        });
        // …and `[keymap]` likewise.
        config
            .keymap
            .entry("normal".into())
            .or_default()
            .insert("Q".into(), "enter-command".into());
        let mut keys = Vec::new();
        dotted(&config.as_table(), "", &mut keys);
        for prefix in SAFETY_KEYS {
            assert!(
                keys.iter().any(|k| k == prefix),
                "`{prefix}` is not a setting: {keys:?}"
            );
        }
    }

    /// **`nan` must not silently disable the high-entropy net.**
    ///
    /// `bits >= f64::NAN` is false for every run, so a single typo in a config
    /// file switches off a security control while `redact_high_entropy = true`
    /// still reads as enabled. TOML 1.0 accepts `nan` and `inf` as float
    /// literals, so this is reachable by writing them.
    #[test]
    fn an_out_of_domain_entropy_threshold_is_reset_and_reported() {
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            let mut cfg = Config::default();
            cfg.secrets.entropy_bits = bad;
            let said = cfg.clamp();
            assert!(
                cfg.secrets.entropy_bits.is_finite() && cfg.secrets.entropy_bits > 0.0,
                "{bad} survived clamp as {}",
                cfg.secrets.entropy_bits
            );
            assert!(
                said.iter().any(|s| s.contains("entropy_bits")),
                "{bad} was reset silently: {said:?}"
            );
        }
        // A sane value is left alone and reported about.
        let mut ok = Config::default();
        assert!(
            !ok.clamp().iter().any(|s| s.contains("entropy_bits")),
            "the shipped default should not report an adjustment"
        );
    }

    /// An `entropy_min_run` below the arithmetic floor does nothing, and saying
    /// nothing about it lets a user believe they narrowed the net.
    #[test]
    fn an_inert_entropy_min_run_is_reported() {
        let mut cfg = Config::default();
        cfg.secrets.entropy_bits = 4.5; // needs >= 23 distinct chars
        cfg.secrets.entropy_min_run = 16;
        let said = cfg.clamp();
        assert!(
            said.iter()
                .any(|s| s.contains("entropy_min_run") && s.contains("no effect")),
            "an inert setting was accepted in silence: {said:?}"
        );
        // The value is reported, not overridden — it is still the user's call.
        assert_eq!(cfg.secrets.entropy_min_run, 16);
        // And the shipped pair is not reported, or the warning is noise.
        let mut shipped = Config::default();
        assert!(
            !shipped
                .clamp()
                .iter()
                .any(|s| s.contains("entropy_min_run")),
            "the shipped defaults must be a valid pair"
        );
    }

    #[test]
    fn clamp_brings_height_into_range() {
        let mut high = Config::default();
        high.tui.prompt.height = u16::MAX;
        let adj = high.clamp();
        assert_eq!(high.tui.prompt.height, MAX_PROMPT_HEIGHT);
        assert_eq!(adj.len(), 1, "an out-of-range value reports one adjustment");

        let mut low = Config::default();
        low.tui.prompt.height = 0;
        low.clamp();
        assert_eq!(low.tui.prompt.height, MIN_PROMPT_HEIGHT);

        let mut ok = Config::default();
        ok.tui.prompt.height = 20;
        assert!(ok.clamp().is_empty(), "in-range value is untouched");
        assert_eq!(ok.tui.prompt.height, 20);
    }

    /// **An arming delay no key could get past is brought back, and said**
    /// (review stop 6, B 4): each answer key pressed too soon starts the wait
    /// again, so `u64::MAX` left a call answerable only by closing its pane.
    #[test]
    fn clamp_bounds_the_approval_delay() {
        let mut long = Config::default();
        long.tui.approval_delay_ms = u64::MAX;
        let said = long.clamp();
        assert_eq!(long.tui.approval_delay_ms, MAX_APPROVAL_DELAY_MS);
        assert!(
            said.len() == 1 && said[0].contains("approval_delay_ms"),
            "{said:?}"
        );
        let mut at = Config::default();
        at.tui.approval_delay_ms = MAX_APPROVAL_DELAY_MS;
        assert!(at.clamp().is_empty(), "the bound itself is kept");
    }

    /// **A decision pane of no rows is brought to one, and said** (review
    /// stop 8, B 5): `0` opened its borders with nothing in them, every
    /// answer refused for want of room. One row is the least it draws.
    #[test]
    fn clamp_gives_a_decision_pane_a_row() {
        let mut none = Config::default();
        none.tui.approval_max_height = 0;
        let said = none.clamp();
        assert_eq!(none.tui.approval_max_height, 1);
        assert!(
            said.len() == 1 && said[0].contains("approval_max_height"),
            "{said:?}"
        );
        let mut one = Config::default();
        one.tui.approval_max_height = 1;
        assert!(one.clamp().is_empty(), "one row is kept");
    }

    /// **`[tui.fold]` reads its four spellings** — `"shut"`, `"open"`,
    /// `"manual"`, `{ shut_over = N }` — per kind, the others as shipped; a
    /// value it cannot read says what it can, and a misspelt key in the
    /// table is refused, not dropped.
    #[test]
    fn tui_fold_reads_open_shut_and_shut_over() {
        assert_eq!(Config::default().tui.fold, FoldConfig::SHIPPED);
        let read = |text: &str| Config::parse(text).map(|c| c.tui.fold);
        let shipped = FoldConfig::SHIPPED;
        assert!(shipped.tool.has_a_fold() && shipped.thinking.has_a_fold());
        assert!(
            !shipped.reply.has_a_fold() && !shipped.message.has_a_fold(),
            "a reply and your message: none until made"
        );
        let reply = read("[tui.fold]\nreply = \"shut\"\nmessage = \"open\"\n").unwrap();
        assert_eq!(reply.reply, FoldStart::Fixed(Folded::Shut));
        assert!(reply.message.has_a_fold() && !reply.message.shuts(9));
        let manual = read("[tui.fold]\ntool = \"manual\"\n").unwrap();
        assert!(!manual.tool.has_a_fold() && !manual.tool.shuts(9));
        let fold = read("[tui.fold]\nthinking = \"open\"\ntool = { shut_over = 40 }\n").unwrap();
        assert_eq!(fold.thinking, FoldStart::Fixed(Folded::Open));
        assert_eq!(fold.tool, FoldStart::Over(ShutOver { shut_over: 40 }));
        assert!(!fold.tool.shuts(40) && fold.tool.shuts(41), "more than N");
        let one = read("[tui.fold]\ntool = \"open\"\n").unwrap();
        assert_eq!(
            one.thinking,
            FoldStart::Fixed(Folded::Shut),
            "unsaid: shipped"
        );
        let wrong = read("[tui.fold]\nthinking = \"closed\"\n")
            .unwrap_err()
            .to_string();
        assert!(
            wrong.contains("shut_over = N"),
            "says what it reads: {wrong}"
        );
        assert!(read("[tui.fold]\ntool = { shut_ovr = 40 }\n").is_err());
        assert!(read("[tui.fold]\nreplies = \"shut\"\n").is_err());
    }
}

#[cfg(test)]
mod reporting_tests {
    use super::*;

    /// **A config that does not parse must SAY SO**, because the consequence is
    /// silent: the whole file is dropped and the app runs on shipped defaults,
    /// so the user's `[secrets]` paths and names are not protected. (This said
    /// their "`[[rules]]` block" was dropped — permission rules are not
    /// configurable yet.) `:config edit` already reports this on screen, so a user learns
    /// "it tells me when my config is bad" — and then edits the file in vim
    /// outside the app and is told nothing (Reviewer F).
    #[test]
    fn a_config_that_does_not_parse_is_reported_not_just_logged() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[tui]\nprompt = { height = \"tall\" }\n").expect("write");

        let (config, said) = Config::load_path_reporting(&path);
        assert_eq!(
            config.tui.prompt.height,
            Config::default().tui.prompt.height,
            "premise: it fell back to defaults"
        );
        assert!(
            said.iter().any(|s| s.contains("NOT LOADED")),
            "the user must be told the file was not applied: {said:?}"
        );
        // What it COSTS, stated as what is true: this asserted "more
        // permissive than your rules", and there are no user rules yet.
        assert!(
            said.iter()
                .any(|s| s.contains("[secrets] settings") && s.contains("NOT in effect")),
            "and told what that costs: {said:?}"
        );
        assert!(
            !said.iter().any(|s| s.contains("your rules")),
            "and not about rules the user cannot have written: {said:?}"
        );
    }

    /// The quiet paths stay quiet: a good config says nothing, and so does a
    /// missing one — that is the normal first run, not a problem.
    #[test]
    fn a_good_or_absent_config_says_nothing() {
        let dir = tempfile::tempdir().expect("tmp");
        let good = dir.path().join("good.toml");
        std::fs::write(&good, "[tui]\nmouse = false\n").expect("write");
        assert!(Config::load_path_reporting(&good).1.is_empty());

        let missing = dir.path().join("nope.toml");
        assert!(Config::load_path_reporting(&missing).1.is_empty());
    }

    /// An unknown key is a typo the user wants to know about — it is silently
    /// doing nothing otherwise.
    #[test]
    fn an_unknown_key_is_reported() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[tui]\nmouse = false\n\n[nonsense]\nx = 1\n").expect("write");
        let (_, said) = Config::load_path_reporting(&path);
        assert!(
            said.iter().any(|s| s.contains("nonsense")),
            "the typo must be named: {said:?}"
        );
    }

    /// **A retired key still loads, and says it is retired** — `[secrets]
    /// keep_marked`, which the template wrote from 10-19 to 10-23: refused
    /// by `deny_unknown_fields`, the whole file would fall back to defaults,
    /// every other setting in it lost without a word.
    #[test]
    fn a_retired_key_loads_and_is_said_retired() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[tui]\nmouse = false\n\n[secrets]\nkeep_marked = true\n",
        )
        .expect("write");
        let (config, said) = Config::load_path_reporting(&path);
        assert!(!config.tui.mouse, "the rest of the file applies");
        assert!(!config.secrets.password_protect, "not read as protection");
        assert!(
            said.iter()
                .any(|s| s.contains("keep_marked") && s.contains("retired")),
            "{said:?}"
        );
        assert!(
            !super::set::option_names()
                .iter()
                .any(|n| n.contains("keep_marked")),
            "never offered"
        );
    }
}

#[cfg(test)]
mod unusable_setting_tests {
    use super::*;

    fn notes_for(toml: &str) -> Vec<String> {
        let (_, notes) = Config::parse_ready(toml).expect("parses");
        notes
    }

    /// **An `entropy_bits` no run can reach is the `nan` off switch with an
    /// ordinary number** — the check is off while the config says on. Named as
    /// OFF, and instead of the `entropy_min_run` note, which blamed the wrong
    /// knob (and at 64+ printed a saturated 18446744073709551615).
    #[test]
    fn an_unreachable_entropy_threshold_is_named_as_off() {
        for bits in ["7.0", "64.0", "1024.0", "1e308"] {
            let notes = notes_for(&format!("[secrets]\nentropy_bits = {bits}\n"));
            assert!(
                notes
                    .iter()
                    .any(|n| n.contains("can never be reached") && n.contains("OFF")),
                "{bits}: {notes:?}"
            );
            assert!(
                !notes.iter().any(|n| n.contains("entropy_min_run")),
                "{bits}: the min_run note blames the wrong knob: {notes:?}"
            );
        }
    }

    /// The ceiling is the alphabet's, not a round number: 6.0 is reachable
    /// (log2 of 66 is ~6.04), and saying OFF about it would be a false alarm.
    #[test]
    fn a_high_but_reachable_threshold_is_not_called_off() {
        let notes = notes_for("[secrets]\nentropy_bits = 6.0\nentropy_min_run = 64\n");
        assert!(
            !notes.iter().any(|n| n.contains("never be reached")),
            "{notes:?}"
        );
    }

    /// Turned off on purpose, it is not a problem to report.
    #[test]
    fn a_disabled_pass_is_not_reported_as_unreachable() {
        let notes = notes_for("[secrets]\nredact_high_entropy = false\nentropy_bits = 64.0\n");
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// An inert `entropy_min_run` note says the value was KEPT — it used to be
    /// suffixed "clamped to range" along with every other note.
    #[test]
    fn a_kept_value_does_not_say_clamped() {
        let notes = notes_for("[secrets]\nentropy_min_run = 16\n");
        let note = notes
            .iter()
            .find(|n| n.contains("entropy_min_run"))
            .expect("reported");
        assert!(note.contains("kept as written"), "{note}");
        assert!(!note.contains("clamped"), "{note}");
    }

    /// An exclusion that matches no shipped name excludes nothing, and says so;
    /// one written in the user's own spelling of a shipped name is fine.
    #[test]
    fn an_exclusion_that_matches_nothing_is_reported() {
        let notes = notes_for("[secrets]\nnames_exclude = [\"tokne\", \"API_KEY\"]\n");
        assert!(
            notes
                .iter()
                .any(|n| n.contains("\"tokne\" matches no shipped name")),
            "{notes:?}"
        );
        assert!(!notes.iter().any(|n| n.contains("API_KEY")), "{notes:?}");
    }
}
