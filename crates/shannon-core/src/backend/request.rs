//! The input to a backend turn.
//!
//! A struct rather than positional arguments so per-turn knobs can be
//! added without a breaking change to the `Backend` trait. design.md
//! §1 makes this load-bearing: a Turn's identity is derived from the
//! model, sampling parameters, system prompt, and tool definitions in
//! effect — every one of those becomes a field here over time, and a
//! struct absorbs them additively where a positional signature could
//! not.
//!
//! M1 populates only `messages` (always) and leaves `model` /
//! `max_tokens` as `None`, meaning "use the backend instance's
//! default." The override path exists today so "switch model
//! mid-session" is expressible the moment the UI wants it.

use serde::Serialize;

use crate::Message;

/// The system prompt sent with every turn that is offered tools.
///
/// **There was none at all until 2026-08-10.** The model was handed a `bash`
/// tool and no context whatsoever: not that it was attached to a terminal on
/// somebody's machine, not which directory, not that looking at the machine
/// means running a command rather than recalling one. A bare tool with no
/// framing is a tool most models decline in favour of answering from memory,
/// which is what the demo saw.
///
/// Deliberately short. It says what the model is, where it is, and the two
/// things about this program it cannot infer — that commands face a gate, and
/// that its reply is rendered as markdown in a terminal. Everything else is the
/// user's to say. A long system prompt is a standing instruction competing with
/// the one the user just typed.
///
/// `{cwd}` is substituted by [`system_prompt`].
///
/// Private: the shipped text is not API, only the rendered result is.
const SYSTEM_TEMPLATE: &str = "You are Shannon Code, a coding agent running in a terminal on the user's own machine. The working directory is {cwd}.

You can see and change this machine only through your tools: shell commands with `bash`, and files with `Read`, `Edit` and `Write`. You have no other access. Read a file with `Read` and change it with `Edit` or `Write`. Do not edit or read with cat, sed or a heredoc through bash unless the user explicitly asks: the user sees the change itself, and the result says where it landed. When a question is about what is actually here, run a command and look. Prefer one command that answers the question over several that approach it. When the skills list Shannon gives you has one that fits the task, load it with the `skill` tool and follow it.

Use the `mark` tool to mark part of your conversation that a user may miss, such as a question or important context delivered mid turn. Reference marks you've created at the end of the turn, so the user sees them. Try to adapt your marks to fit the user's style. Remember their feedback.

Your replies are rendered as markdown in a terminal pane, so keep them tight: short paragraphs, fenced code blocks with a language, no decorative headings for a two-line answer.

When writing code be elegant and follow unix philosophy. Code must be understood with the context of other files. Understand the call graph before adding new code. Ensure that tests are generalizable and not prescriptive.";

/// **What Shannon does around the model that the model cannot infer**: the
/// values held back from it, and where the user's manual is. Steven, 10-20
/// (image 22): a model handed `{{secret.f97e}}` replied it "reached me as
/// literal text… the substitution step may be what's broken", and "it
/// doesn't know where the help files are". One text for both backends —
/// appended to Shannon's own prompt ([`system_prompt`]) and given to Claude
/// Code as `--append-system-prompt` (`claude_code::arguments`), which keeps
/// Claude Code's own — so the two cannot be told different things. Short on
/// purpose, as the template is; more is to come (TODO).
const SHANNON_NOTES: &str = "Shannon, the program running this conversation, holds some values back from you. A handle such as `{{secret.1a2b}}` or `{{gh.4f19}}` (a lowercase name, a dot and four hex digits, in double braces) stands for a value the user marked secret, and `{{a:repo}}` for one they named: write the handle exactly as it is wherever the value belongs in a command, and Shannon puts the value in when the command runs. This is deliberate, not a template that failed. The user can see the value and you do not.

The user reads Shannon's manual with `:help` in Shannon (for example `:help secrets`); point them there for questions about Shannon itself and help instruct them where you can.";

/// **What Shannon does around the model** — its handles, its manual — the
/// paragraph [`system_prompt`] ends with, for a backend that builds its own
/// prompt (Claude Code's `--append-system-prompt`).
pub fn shannon_notes() -> &'static str {
    SHANNON_NOTES
}

/// The system prompt, with where Shannon started filled in — and, when one
/// was vouched for, the scratch directory (`crate::scratch`) — then what
/// Shannon does around the model ([`shannon_notes`]).
///
/// A function rather than a `const` because the directory is the one part that
/// cannot be known at compile time — and it is the part that stops the model
/// guessing at paths. Falls back to a plain statement when the cwd is
/// unreadable, rather than interpolating something misleading. Told by the
/// caller (`App::start`), not read again here: one answer to where Shannon
/// started (review of 716f4f9, C-M5's class).
///
/// The scratch directory is named because it is approved as the start is
/// (Steven, 10-06): a model that does not know it reaches for `/tmp`, which
/// asks.
pub fn system_prompt(start: Option<&std::path::Path>, scratch: Option<&str>) -> String {
    let cwd = start.map_or_else(
        || "unknown (the process could not read it)".to_string(),
        |dir| dir.display().to_string(),
    );
    let prompt = SYSTEM_TEMPLATE.replace("{cwd}", &cwd);
    let prompt = match scratch {
        Some(dir) => format!(
            "{prompt}\n\nFor temporary files, use {dir} — it is yours, and works as the working \
             directory does; elsewhere in /tmp asks."
        ),
        None => prompt,
    };
    format!("{prompt}\n\n{SHANNON_NOTES}")
}

/// A tool the model may call — provider-neutral. Serialises to the Anthropic
/// `tools` entry shape (`{name, description, input_schema}`); a different backend
/// maps the same fields to its own wire form. `input_schema` is a JSON Schema
/// object describing the tool's arguments.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

impl ToolDef {
    /// The tools a main-session turn is offered, as a display string. One place
    /// names them, so a surface that reports "what was in scope" cannot drift
    /// from what was actually sent.
    pub fn offered_names() -> String {
        let always: Vec<String> = Self::foreground().into_iter().map(|t| t.name).collect();
        format!(
            "{}, and {} where a skill is found",
            always.join(", "),
            crate::message::SKILL_TOOL
        )
    }

    /// **The one tool set**, around the `bash` a caller describes: the file
    /// tools ([`files`](Self::files), told whether anyone watches), then
    /// `mark`. Its three callers name only what differs — which `bash`,
    /// watched or not — so the sets cannot drift apart in what or in order
    /// (the order is the cache prefix's, so one set per session).
    fn offered(bash: ToolDef, unattended: bool) -> Vec<ToolDef> {
        std::iter::once(bash)
            .chain(Self::files(unattended))
            .chain(std::iter::once(Self::mark()))
            .collect()
    }

    /// The tool set offered on the foreground conversation. (`skill` joins
    /// it where a skill is found: [`skill`](Self::skill).)
    pub fn foreground() -> Vec<ToolDef> {
        Self::offered(Self::bash(), false)
    }

    /// The foreground tool set, told about the secrets this session holds.
    ///
    /// The handles go in the tool *description* rather than a system prompt
    /// because that is what they are: part of the contract for calling this tool.
    /// A model that is not told cannot spend a secret at all — which is also the
    /// containment property, since a handle it was never given will not resolve.
    pub fn foreground_with(aliases: &crate::Aliases) -> Vec<ToolDef> {
        Self::offered(Self::bash().with_secrets(aliases), false)
    }

    /// The tool set for **unattended** work — a `:child` or a `:Send` reply.
    ///
    /// A different description, because the foreground's is a *lie* here: it
    /// tells the model "every command is reviewed by the user before it runs",
    /// and for a job neither clause holds — a safe read is never reviewed, and
    /// a blocked call returns nothing at all, because the job stops.
    ///
    /// `secrets.md` §10c says a hint to the model is a real control and that
    /// the model is a participant in the posture. The corollary is that a wrong
    /// hint is a real *anti*-control: an optimizer told a human is checking has
    /// no reason to restrain itself, and one told nothing comes back has every
    /// reason to pick the command that works first time (Reviewer E).
    pub fn unattended_with(aliases: &crate::Aliases) -> Vec<ToolDef> {
        let mut bash = Self::bash().with_secrets(aliases);
        // Line continuations, not a bare multi-line literal: `rustfmt` bakes
        // the source indentation into the string, and this one had two
        // fourteen-space runs mid-sentence — in the one artifact whose entire
        // job is to be read carefully (Reviewer E, round 3).
        bash.description = "Run a shell command on the user's machine and return \
             its combined output. NOBODY IS WATCHING this session: commands that \
             only observe state run immediately and unreviewed, and anything \
             else does not run at all — this session stops and waits for a \
             human, and you will receive no result for it. Prefer commands that \
             only read; a command that changes anything will simply end your turn."
            .to_string();
        // A mark runs nothing, so it is the same tool watched or not.
        Self::offered(bash, true)
    }

    /// **The `skill` tool: load one of the skills Shannon listed** — by name,
    /// never a path, so the model reads only a file your config's locations
    /// found (`knowledge::find`); its text comes back as the result. Answered
    /// at once, without the gate: it runs nothing and reads a file you named,
    /// so it is the same watched or not. Offered beside `bash` only where a
    /// skill is found — a tool that can only fail is noise in every request
    /// (the app's `tool_request`). `docs/DesignDocs/memory-and-skills.md`.
    pub fn skill() -> Self {
        Self {
            name: crate::message::SKILL_TOOL.to_string(),
            description: "Load a skill, instructions for a kind of task, by its name, from \
                 the skills list Shannon gave you (the `skills` context). Load one when its \
                 description fits the task, then follow it. Its text comes back as the result."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The skill's name, as the list gives it.",
                    },
                    "arguments": {
                        "type": "string",
                        "description": "What to do it on, if anything; the skill's text says where it goes.",
                    },
                },
                "required": ["name"],
            }),
        }
    }

    /// **The `mark` tool: mark a place the user can jump back to** — the
    /// agent's marks (`navigation.md` §7, NQ4; Steven, 10-27: "we will do
    /// testing to see if the model reliably can mark conversations"). Here,
    /// where the call falls, or an earlier place found by a quoted phrase.
    /// Shannon names it — the next free letter — and the result says which,
    /// so the reply can name a mark that exists (NQ5). Answered at once,
    /// without the gate: it runs nothing. The when and the naming are said
    /// here, the contract for calling it, sent with every request that
    /// offers it.
    pub fn mark() -> Self {
        Self {
            name: crate::message::MARK_TOOL.to_string(),
            // Steven's draft (10-28), for testing whether the model marks
            // unprompted; "latter" read as "letter", and the last paragraph
            // (here, or `at` a quote) added so the model knows the input.
            description: "Mark a place in a conversation so that the user may easily navigate \
                 to its coordinates using the '`' key and a letter. Do not ask for the user's \
                 permission. Examples of where to place marks include, but are not limited \
                 to: a question asked mid turn, important context you want the user to read, \
                 the start of a long section, or a section a user asks you to mark.\n\nAt the \
                 end of your turn report to the user the marks you want them to review, even \
                 if they have not been made this turn. Inform the user simply with the \
                 backtick and the associated letter.\n\nWithout `at`, the mark is placed \
                 where this call falls, after what you have written so far. With `at`, it \
                 marks an earlier place: `quote` is a short phrase copied exactly from one \
                 line, searched from this turn back (not the user's message of this turn, \
                 nor your thinking), \
                 or in the turn `turns_ago` names. The result says where it landed."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "note": {
                        "type": "string",
                        "description": "What the place is, in a few words. The user reads it beside the mark.",
                        // The TUI refuses longer (`MAX_NOTE`).
                        "maxLength": 200,
                    },
                    "at": {
                        "type": "object",
                        "description": "An earlier place, instead of here.",
                        "properties": {
                            "quote": {
                                "type": "string",
                                "description": "A short phrase copied exactly from one line, as it appears there.",
                                // The TUI refuses longer unsearched (`MAX_QUOTE`).
                                "maxLength": 200,
                            },
                            "turns_ago": {
                                "type": "integer",
                                "minimum": 0,
                                "description": "The turn to look in: 0 is this one, 1 the one before. Leave it out to search from this turn back.",
                            },
                        },
                        "required": ["quote"],
                    },
                },
                "required": ["note"],
            }),
        }
    }

    /// The `bash` tool: run a shell command — every invocation routed through
    /// the permission gate before it runs. (The others run nothing:
    /// [`skill`](Self::skill), [`mark`](Self::mark).)
    pub fn bash() -> Self {
        Self {
            name: crate::message::BASH_TOOL.to_string(),
            // **What is actually true at this gate.** This said "Every command
            // is reviewed by the user before it runs", which is false for
            // every read the policy already allows and for every command an
            // allow rule covers (`A` teaches them) — both auto-run with no
            // pane and no keypress. §7
            // corrected exactly this sentence for jobs and landed the fix on
            // `unattended_with` alone: a hint to the model is a real control,
            // so a wrong one is a real anti-control, and an optimizer told a
            // human is checking has no reason to restrain itself. A STRING is
            // a site too (Reviewer E, round 3).
            // And it said "anything else is shown to the user first" while a
            // redirect into a file inside the working directory is allowed by
            // default (Q7) and runs with no pane — measured 11-02, held by
            // `what_runs_unreviewed_is_what_the_descriptions_say`.
            description: "Run a shell command on the user's machine and return \
                 its combined output. Commands that only observe state, ones that \
                 write files inside the working directory, and ones the user has \
                 already approved may run immediately and unreviewed; anything else \
                 is shown to the user first, who may allow it, edit it, or deny it. \
                 A denied command returns an error result."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command line to execute.",
                    },
                },
                "required": ["command"],
            }),
        }
    }

    /// **The file tools — `Read`, `Edit`, `Write`** (`files`;
    /// `file-tools.md`): Claude Code's names and required arguments, run by
    /// Shannon. Each description says what is true at this gate, as
    /// `bash`'s does: a read or a change inside the working directory runs
    /// unreviewed unless a rule says otherwise (Q7, `decide.rs`
    /// `path_default`); `unattended` says what is true for a job, where only
    /// a read inside runs (`safe_unattended`).
    ///
    /// In every set offered (`ToolDef::offered`); the app admits them
    /// by `App::admit_file_call`.
    pub fn files(unattended: bool) -> Vec<ToolDef> {
        let gate = |change: &str| match unattended {
            false => format!(
                "{change} inside the working directory runs at once, unreviewed, unless one \
                 of the user's rules says otherwise; anything else is shown to the user \
                 first, who may allow it, edit it, or deny it."
            ),
            true => "NOBODY IS WATCHING this session: anything but a read inside the \
                     working directory does not run at all — this session stops and waits \
                     for a human, and you will receive no result for it."
                .to_string(),
        };
        let path = serde_json::json!({
            "type": "string",
            "description": "The file: absolute, or relative to the working directory. `~` is not expanded.",
        });
        vec![
            Self {
                name: crate::files::READ.to_string(),
                description: format!(
                    "Read a text file on the user's machine. Its lines come back numbered, \
                     `N<tab>text`, from line 1 or from `offset`, at most `limit` of them; a \
                     file past 2,000 lines or 256 KiB comes back in part, saying where to read \
                     on. Use this rather than cat, head or sed -n through bash: Edit matches \
                     text exactly as Read shows it, without the number prefix. Credentials in \
                     the file are replaced before you see it. It reads files, not \
                     directories: list those with ls. Lines you have read already, the \
                     file unchanged since, are not sent again: the result says your earlier \
                     one is still current. {}",
                    gate("A read")
                ),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": path,
                        "offset": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "The line to start from, 1 being the first. Leave it out to start at the top.",
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "How many lines to read. Leave it out for the whole file, up to the cap.",
                        },
                    },
                    "required": ["file_path"],
                    "additionalProperties": false,
                }),
            },
            Self {
                name: crate::files::EDIT.to_string(),
                description: format!(
                    "Replace text in a file on the user's machine: `old_string` becomes \
                     `new_string`, matched exactly — whitespace, indentation and line breaks \
                     included, and never the `N<tab>` prefix a Read shows. `old_string` must \
                     occur exactly once, unless `replace_all` is true, which changes every \
                     occurrence; a string found nowhere, or more than once, is refused and \
                     nothing changes. Use this rather than sed, perl or a heredoc through \
                     bash: the user sees the change itself, and the result names the lines it \
                     changed. A file with Windows line endings keeps them. Read the file \
                     first: an Edit of a file you have not read in this conversation, or \
                     one changed since you read it, is refused and says so. {}",
                    gate("A change")
                ),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": path,
                        "old_string": {
                            "type": "string",
                            "description": "The text to replace, exactly as it is in the file.",
                        },
                        "new_string": {
                            "type": "string",
                            "description": "What replaces it; different from old_string.",
                        },
                        "replace_all": {
                            "type": "boolean",
                            "description": "Replace every occurrence, not one. Default false.",
                        },
                    },
                    "required": ["file_path", "old_string", "new_string"],
                    "additionalProperties": false,
                }),
            },
            Self {
                name: crate::files::WRITE.to_string(),
                description: format!(
                    "Write a whole file on the user's machine: make it, or replace everything \
                     in it. To change part of a file use Edit, which sends only the change. \
                     The file's directory must already exist (make it with mkdir -p through \
                     bash). A file replaced keeps its line endings, and is Read first, as \
                     for Edit. {}",
                    gate("A write")
                ),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": path,
                        "content": {
                            "type": "string",
                            "description": "The whole file.",
                        },
                    },
                    "required": ["file_path", "content"],
                    "additionalProperties": false,
                }),
            },
        ]
    }

    /// Append the session's secret handles to this tool's description.
    ///
    /// Tells the model three things it cannot infer: that the handles exist, that
    /// writing one is how it uses the secret, and that the value is genuinely
    /// unavailable to it. The last matters — a model that believes it *could* see
    /// the value will waste a turn trying, or worse, invent one.
    fn with_secrets(mut self, aliases: &crate::Aliases) -> Self {
        let listed = aliases
            .spendable()
            .map(|s| format!("  {} — {}", s.handle, s.source))
            .collect::<Vec<_>>()
            .join("\n");
        // Handles the user marked but did NOT lend for use. Naming them stops a
        // wasted turn spent trying to use one, and stops the model concluding
        // the substitution was a mistake it should work around.
        let kept: Vec<String> = aliases
            .secrets()
            .filter(|s| !s.spendable)
            .map(|s| s.handle.clone())
            .collect();
        if listed.is_empty() && kept.is_empty() {
            return self;
        }
        if listed.is_empty() {
            self.description.push_str(&format!(
                "\n\nSome values in this conversation were replaced by placeholders \
                 to keep them off the wire. They cannot be used in a command and \
                 their values are not available to you; treat them as opaque and \
                 reason about them as such: {}",
                kept.join(", ")
            ));
            return self;
        }
        self.description.push_str(&format!(
            "\n\nThe user has lent this session secrets you may USE but never SEE. \
             Write the handle verbatim where the value belongs; it is replaced with \
             the real credential at execution, after the user approves the command. \
             The values are not available to you by any other means, and a handle \
             not listed here will refuse to run.\n{listed}"
        ));
        if !kept.is_empty() {
            self.description.push_str(&format!(
                "\n\nThese are placeholders too, but are NOT usable in a command — \
                 they were kept off the wire rather than lent: {}",
                kept.join(", ")
            ));
        }
        self
    }
}

/// One request to a backend: the conversation so far, plus optional
/// per-turn overrides. Build with `SendRequest::new(messages)` and
/// chain setters for any overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct SendRequest {
    /// Full conversation history, oldest first, ending with the user
    /// message this turn responds to.
    pub messages: Vec<Message>,
    /// Model override. `None` → the backend's configured default.
    pub model: Option<String>,
    /// Output-token cap override. `None` → the backend's default.
    pub max_tokens: Option<u32>,
    /// Tools the model may call this turn. Empty → a plain (no-tool) turn, so
    /// existing conversations are unaffected.
    pub tools: Vec<ToolDef>,
    /// The system prompt. `None` → none is sent.
    ///
    /// **Nothing sent one until now**, and the module doc above has promised
    /// this field since M1. The cost was not abstract: a bare `bash` tool with
    /// no framing is a tool most models decline in favour of answering from
    /// memory, because nothing has told them they are attached to a terminal on
    /// somebody's machine, which machine, or that looking at it means running a
    /// command.
    ///
    /// It rides ahead of the messages in the cached prefix (the API orders
    /// `tools` → `system` → `messages`), so a stable one costs nothing after
    /// the first turn.
    pub system: Option<String>,
    /// **The provider's own session to continue**, for a provider that keeps
    /// the conversation itself (`claude -p --resume <id>`): then only the
    /// newest message is sent. `None` → the whole history goes in.
    pub resume: Option<String>,
    /// How much work the model is asked to put in — the API's
    /// `output_config.effort`, `claude --effort`. `None` → nothing is sent,
    /// and the model works at its own default; a request without one is
    /// byte-identical to one from before the field existed.
    pub effort: Option<crate::Effort>,
    /// **Whether a provider that finds instruction files of its own reads
    /// them** — Claude Code's CLAUDE.md, which `[memory] claude = "off"`
    /// turns off. `true` by default: what the provider does unasked.
    pub own_instructions: bool,
    /// **Whether a provider that keeps memories of its own keeps them** —
    /// Claude Code's auto memory (its prompt's memory section, and its
    /// `MEMORY.md`), which `[memory] claude = "off"` or `memory = off` turns
    /// off. `true` by default: what the provider does unasked.
    pub own_memory: bool,
    /// **Whether a provider that compacts its own context may do so unasked**
    /// — Claude Code's auto-compaction near its window's end, which
    /// `[claude_code] auto_compact = false` turns off (its manual `/compact`
    /// stays). `true` by default: what the provider does unasked.
    pub auto_compact: bool,
    /// **Whether a provider that runs work in the background may** — Claude
    /// Code's background agents and Bash's `run_in_background`, which
    /// `[claude_code] background_tasks = false` turns off. `true` by
    /// default: what the provider does unasked — though a background
    /// agent's calls are refused, as never named in the turn's output.
    pub background_tasks: bool,
    /// **What a provider that runs commands itself (`claude`) is given of
    /// Shannon's environment** ([`ProviderEnv`]). Set at the one request door
    /// from the live config and table; by default, everything less Shannon's
    /// own key.
    pub env: ProviderEnv,
    /// **What thinking is asked for** — `[turn] thinking` and
    /// `thinking_budget`, set at the one request door from the live config.
    /// The API backend reads the model's category to choose the request
    /// (`models::thinking_kind`); Claude Code keeps its own.
    pub thinking: ThinkingAsk,
}

/// **What a provider that runs commands itself (`claude`) is given of
/// Shannon's environment** — one value for both of its spawns, a turn and
/// the login check (`claude_code::claude_command`), so the login a check
/// reports is the one a turn runs under.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderEnv {
    /// `[claude_code] env = "minimal"`: start from what Shannon's own
    /// commands get (`exec::child_env`), not from everything Shannon holds.
    pub minimal: bool,
    /// `[exec] env`, expanded (`exec::configured_env`): laid over either —
    /// the same `PATH` both backends' commands run with.
    pub set: Vec<(String, String)>,
    /// **Never handed on**, whatever else names them, taken off last: every
    /// variable a secret was lent from (`Aliases::lent_vars` — a variable
    /// left there read the value back, `echo $GITHUB_TOKEN`: review stop 14,
    /// Reviewer E F1) and `[exec] withhold`.
    pub withheld: Vec<String>,
}

/// **What a request asks of thinking** — the `[turn]` knobs, as a request
/// carries them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ThinkingAsk {
    /// What a model that thinks adaptively shows.
    pub display: crate::ThinkingDisplay,
    /// A budgeted model's budget, in tokens; `0` asks for none.
    pub budget: u32,
}

impl ThinkingAsk {
    /// What `turn` asks for.
    pub fn of(turn: &crate::config::TurnSettings) -> ThinkingAsk {
        ThinkingAsk {
            display: turn.thinking,
            budget: turn.thinking_budget,
        }
    }
}

impl SendRequest {
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            messages,
            model: None,
            max_tokens: None,
            tools: Vec::new(),
            system: None,
            resume: None,
            effort: None,
            own_instructions: true,
            own_memory: true,
            auto_compact: true,
            background_tasks: true,
            env: ProviderEnv::default(),
            thinking: ThinkingAsk::default(),
        }
    }

    /// Ask for `effort` — or, with `None`, the model's own default.
    pub fn with_effort(mut self, effort: Option<crate::Effort>) -> Self {
        self.effort = effort;
        self
    }

    /// **Ask for exactly what a turn sealed under `config` records** — its
    /// model, reply ceiling and effort — so a turn's provenance cannot name a
    /// knob its request did not carry. The seal recorded the ceiling for every
    /// turn while one of the app's three request builders sent it.
    pub fn under(self, config: &crate::TurnConfig) -> Self {
        Self {
            model: Some(config.model.clone()),
            max_tokens: config.max_tokens,
            effort: config.effort,
            ..self
        }
    }

    /// Continue the provider's own session `id` (`claude -p --resume`).
    pub fn with_resume(mut self, id: impl Into<String>) -> Self {
        self.resume = Some(id.into());
        self
    }

    /// Set the system prompt for this turn.
    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    /// Offer the model a set of tools for this turn.
    pub fn with_tools(mut self, tools: Vec<ToolDef>) -> Self {
        self.tools = tools;
        self
    }

    /// Override the model for this turn.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Override the output-token cap for this turn.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }
}

#[cfg(test)]
mod description_tests {
    use super::*;

    /// **Neither description claims something the gate does not do.**
    ///
    /// A hint to the model is a real control (`secrets.md` §10c), so a wrong
    /// hint is a real anti-control: an optimizer told a human is checking has
    /// no reason to restrain itself. The foreground's said "Every command is
    /// reviewed by the user before it runs" while `Decision::Allow` — a read,
    /// or a call an allow rule covers — auto-runs with no pane and no
    /// keypress. §7 fixed
    /// that sentence for jobs and left it standing here — a string is a site
    /// too (Reviewer E, round 3).
    ///
    /// Also asserts the strings are not MANGLED: `rustfmt` bakes source
    /// indentation into a bare multi-line literal, and the unattended one
    /// carried two fourteen-space runs mid-sentence.
    #[test]
    fn the_tool_descriptions_do_not_overpromise() {
        let aliases = crate::Aliases::default();
        let foreground = ToolDef::foreground_with(&aliases)[0].description.clone();
        let unattended = ToolDef::unattended_with(&aliases)[0].description.clone();

        assert!(
            !foreground.contains("Every command is reviewed"),
            "the foreground gate auto-runs allowed reads and remembered grants: {foreground}"
        );
        assert!(
            foreground.contains("unreviewed"),
            "…and it must SAY so: {foreground}"
        );
        assert!(
            unattended.contains("NOBODY IS WATCHING"),
            "a job's description must name its own situation: {unattended}"
        );
        for (which, text) in [("foreground", &foreground), ("unattended", &unattended)] {
            assert!(
                !text.contains("   "),
                "{which}: a run of spaces mid-sentence — the literal baked in \
                 its own indentation: {text}"
            );
        }
    }

    /// **What the descriptions say runs unreviewed is what the gate runs** —
    /// measured, not argued (11-02): under the shipped rules a redirect into
    /// a file inside the working directory is allowed (Q7), and `tee` asks
    /// (a writer with no rule). So `bash`'s description says such a write
    /// MAY run unreviewed — it said "anything else is shown to the user
    /// first", an anti-control — and the file tools' say a change inside
    /// runs unreviewed.
    #[test]
    fn what_runs_unreviewed_is_what_the_descriptions_say() {
        use crate::permission::{Decision, RuleSet};
        let rules = RuleSet::defaults().with_start("/home/me/proj");
        let decided = |line: &str| rules.decide_line(line).decision;
        assert_eq!(decided("cat > notes.txt"), Decision::Allow, "the premise");
        assert_eq!(
            decided("cat notes.txt > notes.txt"),
            Decision::Allow,
            "an Edit's reading"
        );
        assert_eq!(
            decided("cat > /tmp/elsewhere"),
            Decision::Ask,
            "outside asks"
        );
        let bash = &ToolDef::foreground_with(&crate::Aliases::default())[0].description;
        assert!(
            bash.contains("write files inside the working directory"),
            "{bash}"
        );
        for tool in ToolDef::files(false) {
            assert!(
                tool.description
                    .contains("inside the working directory runs at once, unreviewed"),
                "{}: {}",
                tool.name,
                tool.description
            );
        }
    }

    /// **The system prompt names every tool it offers** — read from the
    /// sets, not a list beside them: it tells the model it acts "only
    /// through your tools", so a tool it does not name is one the model is
    /// told it lacks (it said `bash` was the only access until 11-02).
    #[test]
    fn the_system_prompt_names_every_tool_offered() {
        let prompt = system_prompt(None, None);
        let aliases = crate::Aliases::default();
        let offered = (ToolDef::foreground().into_iter())
            .chain(ToolDef::unattended_with(&aliases))
            .chain([ToolDef::skill()]);
        for tool in offered {
            assert!(
                prompt.contains(&format!("`{}`", tool.name)),
                "{}: {prompt}",
                tool.name
            );
        }
    }

    /// **A file tool's schema and its reader agree** — built from the
    /// schema, not a list beside it: the required keys alone read as a call,
    /// and so does each optional key with them, through `files::Call::read`;
    /// and each description says what this gate does, as `bash`'s must.
    #[test]
    fn a_file_tools_schema_is_what_its_reader_takes() {
        use serde_json::{Value, json};
        let sample = |schema: &Value| match schema["type"].as_str() {
            Some("integer") => json!(1),
            Some("boolean") => json!(true),
            _ => json!("x"),
        };
        for unattended in [false, true] {
            for tool in ToolDef::files(unattended) {
                let schema = &tool.input_schema;
                let props = schema["properties"].as_object().expect("properties");
                let mut input = serde_json::Map::new();
                for key in schema["required"].as_array().expect("required") {
                    let key = key.as_str().expect("a key");
                    input.insert(key.into(), sample(&props[key]));
                }
                for key in std::iter::once(None).chain(props.keys().map(Some)) {
                    let mut with = input.clone();
                    if let Some(key) = key {
                        with.insert(key.clone(), sample(&props[key]));
                    }
                    let read = crate::files::Call::read(
                        &tool.name,
                        &Value::Object(with.clone()),
                        Some(std::path::Path::new("/w")),
                    );
                    assert!(
                        matches!(read, Some(Ok(_))),
                        "{}: {with:?} → {read:?}",
                        tool.name
                    );
                }
                let text = &tool.description;
                assert!(
                    !text.contains("   "),
                    "{}: baked indentation: {text}",
                    tool.name
                );
                match unattended {
                    false => assert!(text.contains("unreviewed"), "{}: {text}", tool.name),
                    true => assert!(text.contains("NOBODY IS WATCHING"), "{}: {text}", tool.name),
                }
            }
        }
    }
}
