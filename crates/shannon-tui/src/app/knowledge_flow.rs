//! **Memory and skills, wired** — what a turn tells the model beyond the
//! conversation (`shannon_core::knowledge`), from the roots launch set
//! (`App::set_knowledge_roots`). `docs/DesignDocs/memory-and-skills.md`.
//!
//! A child module of `app`, as `models_flow` is.

use std::path::PathBuf;

use super::config_file::tilde;
use super::*;
use crate::shelf::{About, Group, List, Mark, Note, Row, Shelf};
use shannon_core::knowledge::{self, Found, Item, Role, Session, State, Told};

/// **Text a repository chose, as one row shows it** — a control character
/// in a name (a newline in a skill's directory) split a pane's line in two,
/// and every `<Enter>` below it opened its neighbour's file (Reviewer B,
/// round 27). A row is a file now, but a newline still breaks a row.
fn shown(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '\u{fffd}' } else { c })
        .collect()
}

/// A found file's badge.
fn mark(item: &Item) -> Mark {
    match (&item.state, item.role) {
        (State::Refused(why), _) => Mark::NotRead(shown(why)),
        (State::Disabled, _) => Mark::Disabled,
        (State::ClaudeCode, _) => Mark::Claude,
        (_, Role::MemoryIndex) => Mark::Index,
        (State::Loaded, _) => Mark::Sent,
        (State::Listed, _) => Mark::Listed,
    }
}

/// A found file as a row: `name`, what it is for.
fn row(item: &Item, name: &str, about: About) -> Row {
    let about = match about {
        About::Text(text) => About::Text(shown(&text)),
        About::Path(path) => About::Path(shown(&path)),
    };
    Row {
        name: shown(name),
        about,
        path: item.path.clone(),
        mark: mark(item),
    }
}

/// A file's own name.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// **Rows under a heading per place**, in the order the places are met.
fn by_place(rows: impl Iterator<Item = (PathBuf, Row)>) -> Vec<(PathBuf, Vec<Row>)> {
    let mut places: Vec<(PathBuf, Vec<Row>)> = Vec::new();
    for (place, row) in rows {
        match places.iter_mut().find(|(p, _)| *p == place) {
            Some((_, rows)) => rows.push(row),
            None => places.push((place, vec![row])),
        }
    }
    places
}

/// Why a server is not made, edited or removed here.
const SERVERS_ARE_CLAUDES: &str =
    "an MCP server is Claude Code's — `claude mcp` adds, removes and configures one";

/// The most of a file the float's preview reads — a glance is some fifty
/// lines; `<Enter>` opens the rest.
const SHELF_PREVIEW_CAP: usize = 16 * 1024;

/// **The MCP list** — the servers Claude Code said it started (`init`),
/// each by how it stands, and those you disabled that it did not start
/// this time, so `x` can enable them again. Before its first turn here:
/// nothing known yet, and said.
fn servers_list(servers: Option<&[shannon_core::McpServer]>, disabled: &[String]) -> List {
    use shannon_core::McpServer;
    // The gate's own question (`McpServer::named_by`): matched by text, an
    // entry `my_server` refused `my server`'s tools while its row said
    // `connected` (review 29, Reviewer C: D1).
    let off = |name: &str| disabled.iter().any(|d| McpServer::named_by(name, d));
    let row = |name: &str, mark: Mark, about: &str| Row {
        name: shown(name),
        about: About::Text(shown(about)),
        // A server has no file: its name is its identity on the screen.
        path: PathBuf::from(format!("mcp:{name}")),
        mark,
    };
    let mut rows: Vec<Row> = servers
        .unwrap_or_default()
        .iter()
        .map(|s| {
            let mark = match (off(&s.name), s.status.as_str()) {
                (true, _) => Mark::Disabled,
                (false, "connected") => Mark::Connected,
                (false, status) => Mark::Down(shown(status)),
            };
            row(&s.name, mark, &s.status)
        })
        .collect();
    // Before a turn here nothing is known of what starts: said so, not
    // "not started" (review 29).
    let unstarted = match servers {
        None => "not known yet — a turn of Claude Code's names its servers",
        Some(_) => "not started this time",
    };
    for name in disabled {
        let started = servers
            .unwrap_or_default()
            .iter()
            .any(|s| McpServer::named_by(&s.name, name));
        if !started {
            rows.push(row(name.trim(), Mark::Disabled, unstarted));
        }
    }
    let empty = match servers {
        None => {
            "none known yet — Claude Code names its MCP servers when a turn of its starts \
                 here; a session through the API uses none"
        }
        Some(_) => "Claude Code started none — `claude mcp add` adds one",
    };
    List {
        groups: vec![Group {
            place: "Claude Code's MCP servers".into(),
            note: "`claude mcp` configures them".into(),
            rows,
        }],
        empty: empty.into(),
        notes: vec![Note {
            text: "x disables a server: a call to any of its tools is refused. It still \
                   starts, and its tools are still listed to the model."
                .into(),
            caution: false,
        }],
    }
}

/// Why a disabled skill is not loaded — the entry that disables it, which
/// is what `:set skills.disabled-=` takes back (`-={name}` was said for an
/// entry that was a place, and did nothing: review 29, Reviewers A, C).
fn disabled_skill(name: &str, entry: &str) -> String {
    format!(
        "the skill `{name}` is disabled — by `{entry}` in `[skills] disabled` (`:skills` \
         shows it)"
    )
}

/// **The `:set` line taking `entry` out of `[<table>] disabled`**, as it
/// must be typed to mean that one entry: plain where it can be; else a TOML
/// `[…]`, which `:set` takes whole (a `,` splits a plain value), each space
/// and backslash escaped (a space splits a `:set` line).
pub(super) fn take_back_line(table: &str, entry: &str) -> String {
    let plain = !entry.is_empty()
        && !entry.trim_start().starts_with('[')
        && !entry.contains(|c: char| c.is_whitespace() || c == ',' || c == '\\');
    let value = match plain {
        true => entry.to_string(),
        false => toml::Value::Array(vec![toml::Value::String(entry.into())])
            .to_string()
            .chars()
            .flat_map(|c| match c.is_whitespace() || c == '\\' {
                true => vec!['\\', c],
                false => vec![c],
            })
            .collect(),
    };
    format!(":set {table}.disabled-={value}")
}

/// The session's kind, as a listing names it.
fn session_word(session: Session) -> &'static str {
    match session {
        Session::Api => "a session through the API",
        Session::ClaudeCode => "a Claude Code session",
    }
}

/// **`:memories`' list** — the instruction files, then each memory place
/// with its files (the model's own place said, and Claude Code's), then
/// what is not read and why; and what a write does: by `mode`, what it does
/// in fact (`App::write_mode`), beside `set`, what the setting says — and
/// `unvouched`, why the two differ.
fn memories_list(
    found: &Found,
    mode: shannon_core::MemoryMode,
    set: shannon_core::MemoryMode,
    unvouched: &str,
) -> List {
    let refused = |item: &&Item| matches!(item.state, State::Refused(_));
    let mut groups = vec![Group {
        place: "Instructions".into(),
        note: "sent whole with every turn".into(),
        rows: found
            .items
            .iter()
            .filter(|i| i.role == Role::Instructions && !refused(i))
            .map(|i| {
                let place = About::Path(tilde(i.path.parent().unwrap_or(&i.path)));
                row(i, &file_name(&i.path), place)
            })
            .collect(),
    }];
    let write = found.write.as_ref().map(|w| w.dir());
    let memories = found
        .items
        .iter()
        .filter(|i| matches!(i.role, Role::MemoryIndex | Role::Memory) && !refused(i))
        .map(|i| {
            let about = match i.role {
                Role::MemoryIndex => "each line a memory, read when it is relevant".to_string(),
                _ => i.description.clone().unwrap_or_default(),
            };
            let place = i.path.parent().unwrap_or(&i.path).to_path_buf();
            (place, row(i, &file_name(&i.path), About::Text(about)))
        });
    for (place, rows) in by_place(memories) {
        let note = match (write == Some(place.as_path()), &found.write) {
            (true, Some(knowledge::Write::ClaudeCode(_))) => "Claude Code's; the model writes here",
            (true, _) => "the model writes here",
            (false, _) if rows.iter().all(|r| r.mark == Mark::Claude) => "Claude Code's",
            (false, _) => "",
        };
        groups.push(Group {
            place: tilde(&place),
            note: note.into(),
            rows,
        });
    }
    groups.push(Group {
        place: "Not read".into(),
        note: String::new(),
        rows: found
            .items
            .iter()
            .filter(|i| i.role != Role::Skill && refused(i))
            .map(|i| row(i, &file_name(&i.path), About::Text(String::new())))
            .collect(),
    });
    use shannon_core::MemoryMode::{Ask, Auto};
    let whose = match &found.write {
        Some(knowledge::Write::ClaudeCode(_)) => " (Claude Code's own: its prompt says how)",
        _ => "",
    };
    let (writes, caution) = match (write, mode, set) {
        (Some(dir), Auto, _) => (
            format!(
                "Writes: the model writes to {}{whose} unasked — a delete asks",
                tilde(dir)
            ),
            false,
        ),
        (Some(dir), Ask, Auto) => (
            format!(
                "Writes: memory = auto, but {}{whose} is not approved ({unvouched}) — \
                 each write there asks",
                tilde(dir)
            ),
            true,
        ),
        (Some(dir), _, _) => (
            format!(
                "Writes: memory = ask — each write to {}{whose} asks",
                tilde(dir)
            ),
            false,
        ),
        (None, _, _) => (
            "Writes: none — memory = off, or no write location".to_string(),
            false,
        ),
    };
    let mut notes = vec![Note {
        text: writes,
        caution,
    }];
    notes.extend(found.notes.iter().map(|n| Note {
        text: format!("note: {}", shown(n)),
        caution: true,
    }));
    List {
        groups,
        empty: "nothing told — AGENTS.md is read where Shannon starts and above it \
                (:help memory-places)"
            .into(),
        notes,
    }
}

/// **`:skills`' list** — each skill under the place it was found (Claude
/// Code's said), then those not used and why. `places` are `[skills]
/// paths` as written, for the list to say where a skill goes when it has
/// none.
fn skills_list(found: &Found, places: &[String]) -> List {
    // A skill's place: the directory its `<name>/SKILL.md` is in.
    let place = |i: &Item| {
        let dir = i.path.parent().unwrap_or(&i.path);
        match i.path.file_name().is_some_and(|n| n == "SKILL.md") {
            true => dir.parent().unwrap_or(dir).to_path_buf(),
            false => dir.to_path_buf(),
        }
    };
    // Usable, and disabled — under its place still, its badge saying so,
    // for `x` to enable where it was disabled.
    let listed = found
        .items
        .iter()
        .filter(|i| i.role == Role::Skill && !matches!(i.state, State::Refused(_)))
        .map(|i| {
            let about = About::Text(i.description.clone().unwrap_or_default());
            (place(i), row(i, &i.name, about))
        });
    let mut groups: Vec<Group> = by_place(listed)
        .into_iter()
        .map(|(dir, rows)| {
            let note = match rows.iter().all(|r| r.mark == Mark::Claude) {
                true => "Claude Code's",
                false => "",
            };
            Group {
                place: tilde(&dir),
                note: note.into(),
                rows,
            }
        })
        .collect();
    groups.push(Group {
        place: "Not used".into(),
        note: String::new(),
        rows: found
            .items
            .iter()
            .filter(|i| i.role == Role::Skill && matches!(i.state, State::Refused(_)))
            .map(|i| row(i, &i.name, About::Text(String::new())))
            .collect(),
    });
    List {
        groups,
        empty: format!(
            "no skills — a skill is <name>/SKILL.md under {} (:help skills)",
            places.join(", ")
        ),
        notes: vec![Note {
            text: "`:skill <name> [args]` runs one now.".into(),
            caution: false,
        }],
    }
}

impl App {
    /// The kind of session `model` answers in — what decides who reads
    /// Claude Code's own files.
    pub(crate) fn knowledge_session(model: &str) -> Session {
        match shannon_core::backend::claude_code::is_claude_code(model) {
            true => Session::ClaudeCode,
            false => Session::Api,
        }
    }

    /// The kind of session `buffer` is, by the model that answers it.
    pub(crate) fn session_kind(&self, buffer: BufferId) -> Session {
        Self::knowledge_session(&self.session_model(buffer))
    }

    /// **Why a file memory and skills name is not read**, whole, if it is
    /// not — the one judge every such file is put to:
    /// [`found`](Self::found) and [`skill_text`](Self::skill_text). A
    /// credential path, as the gate judges one (`RuleSet::credential_path`);
    /// or a file under a `.shannon/` whose directory is not trusted
    /// (`Trusted::untrusted_shannon`; Steven, 10-13: "Trust should gate
    /// `.shannon/*`") — a repository's `.shannon/skills` among them.
    fn never_read(&self, path: &Path) -> Option<String> {
        if let Some(why) = self.rules.credential_path(&path.display().to_string()) {
            return Some(format!("a credential path ({why}) — never read"));
        }
        let holder = self.trusted.untrusted_shannon(path)?;
        let dir = super::config_file::tilde(&holder);
        Some(format!(
            "under {dir}/.shannon/, which is not trusted — `:trust {dir}` reads it"
        ))
    }

    /// **Text read from disk, as the model may be told it** — redacted as a
    /// command's output is (`secrets::redact`: a value held leaves as its
    /// handle, a credential's shape as `[redacted: …]`). A `password:` line
    /// in an `AGENTS.md` left in clear while `cat AGENTS.md` scrubbed it
    /// (Reviewers A and B, round 27). The door both readers pass: what a turn
    /// tells, and a skill's text. The rule is `App::scrub`'s, called, not
    /// copied (review stop 4, C M6: it was a character-for-character copy).
    fn outbound(&self, text: &str) -> String {
        self.scrub(text)
    }

    /// **Everything `[memory]` and `[skills]` name**, for a `session` —
    /// `None` before launch set the roots (a test app). Every file judged by
    /// the gate's own credential policy, as named and as it resolves
    /// (`knowledge::credential`), so a location never sends a key.
    pub(crate) fn found(&self, session: Session) -> Option<Found> {
        let roots = self.knowledge_roots.as_ref()?;
        Some(knowledge::find(
            &self.config.memory,
            &self.config.skills,
            roots,
            session,
            &|path| self.never_read(path),
        ))
    }

    /// **What a turn must tell the model beyond its prompt**: what the
    /// session's `history` does not already hold of its instructions,
    /// memories and skills (`knowledge::attach`) — through
    /// [`outbound`](Self::outbound), its source masked too (your home is an
    /// alias, and a path names it), so an unchanged file compares equal turn
    /// after turn. The one door every turn starter calls (`start_turn_on`, a
    /// `:child` briefing, `:Send`).
    pub(crate) fn told(&self, session: Session, history: &[Message]) -> Vec<shannon_core::Block> {
        let Some(found) = self.found(session) else {
            return Vec::new();
        };
        let told: Vec<Told> = knowledge::context(&found, self.write_mode(session), session)
            .into_iter()
            .map(|told| Told {
                text: self.outbound(&told.text),
                source: self.aliases.mask(&told.source).0,
                ..told
            })
            .collect();
        // What `claude` reads itself is not gone for being Shannon's no
        // longer: a session switched to Claude Code must not be told its
        // CLAUDE.md was unloaded (Reviewer A, round 27).
        let claudes: Vec<String> = found
            .items
            .iter()
            .filter(|i| i.state == State::ClaudeCode)
            .map(|i| self.aliases.mask(&i.path.display().to_string()).0)
            .collect();
        knowledge::attach(history, &told, &claudes)
    }

    /// **What a write to the memory location does, in fact** — `auto` only
    /// where the location was vouched and its allow is in force; a location
    /// that failed (another account's, a link, open to others) asks, and the
    /// model and `:memories` must say so rather than read the setting
    /// (Reviewers A and C, round 27).
    pub(crate) fn write_mode(&self, session: Session) -> shannon_core::MemoryMode {
        match self.config.memory.mode {
            shannon_core::MemoryMode::Auto if self.memory_write_for(session).is_empty() => {
                shannon_core::MemoryMode::Ask
            }
            mode => mode,
        }
    }

    /// **The text of the skill `name`**, for a `session`, with `args` — the
    /// one reading `:skill` and the `skill` tool share: a skill found along
    /// the locations, never a path, through [`outbound`](Self::outbound).
    /// `None` for Claude Code's own in its session: `claude` loads it itself,
    /// as it reads its CLAUDE.md, and its home's skills are a credential path
    /// Shannon never reads (Reviewers A and C, round 27). Why not, when it is
    /// no skill: the names there are.
    pub(crate) fn skill_text(
        &self,
        session: Session,
        name: &str,
        args: &str,
    ) -> Result<Option<String>, String> {
        let found = self
            .found(session)
            .ok_or("no skills are read here — memory and skills are off")?;
        let Some(item) = found.skill(name) else {
            if let Some(entry) = found.skill_disabled(name) {
                return Err(disabled_skill(name, &entry));
            }
            let names: Vec<&str> = found.skills().map(|s| s.name.as_str()).collect();
            return Err(match names.is_empty() {
                true => format!("no skill named `{name}` — none are found (`:skills`)"),
                false => format!("no skill named `{name}` — the skills: {}", names.join(", ")),
            });
        };
        if item.state == State::ClaudeCode {
            return Ok(None);
        }
        // A refusal names the path, and goes to the model as a tool's
        // result: through the same door as the text (Reviewer E1, round 27).
        let text = knowledge::skill_text(&item.path, args, &|path| self.never_read(path))
            .map_err(|why| self.outbound(&why))?;
        Ok(Some(self.outbound(&text)))
    }

    /// **One of Claude Code's calls on something you disabled**, and why —
    /// a `Skill` call loading a disabled skill; or anything of a disabled
    /// MCP server's: a call to its tools (`mcp__<server>__<tool>`), its
    /// prompt loaded through `Skill` (named so too), a read or listing of
    /// its resources (`ReadMcpResourceTool`, `ListMcpResourcesTool`, which
    /// name it as `server`) — the server matched as Claude Code normalises
    /// it (`McpServer::owns`, `named_by`). Only the tool call was refused
    /// (review 29, Reviewer A: M6). A listing of every server's resources
    /// names no server, and runs: it lists, as its tools still are.
    /// `None`: nothing disabled.
    pub(crate) fn disabled_call(&self, tool: &str, input: &serde_json::Value) -> Option<String> {
        use shannon_core::McpServer;
        let disabled = &self.config.mcp.disabled;
        let named = |text: &str| input[text].as_str().unwrap_or_default().trim();
        let server = match tool {
            "ReadMcpResourceTool" | "ListMcpResourcesTool" => disabled
                .iter()
                .find(|d| McpServer::named_by(named("server"), d)),
            // A prompt runs as `/mcp__<server>__<prompt>` and is LISTED as
            // `/<server>:<prompt>` (Claude Code's MCP docs): either spelling.
            "Skill" => {
                let skill = named("skill").trim_start_matches('/');
                let server = skill.split_once(':').map(|(server, _)| server);
                disabled.iter().find(|d| {
                    McpServer::owns(d, skill) || server.is_some_and(|s| McpServer::named_by(s, d))
                })
            }
            tool => disabled.iter().find(|d| McpServer::owns(d, tool)),
        };
        if let Some(entry) = server {
            return Some(format!(
                "the MCP server `{}` is disabled — `{}` uses it again",
                entry.trim(),
                take_back_line("mcp", entry)
            ));
        }
        // `Found::skill_disabled`, the one question: any spelling Claude Code
        // loads a skill by (`/review`, a plugin's `x:review`, its frontmatter
        // name) — and a usable one of that name is not refused for another.
        let name = input["skill"].as_str().map(str::trim)?;
        let found = self.found(Session::ClaudeCode).unwrap_or_default();
        match tool == "Skill" {
            true => found
                .skill_disabled(name)
                .map(|entry| disabled_skill(name, &entry)),
            false => None,
        }
    }

    /// **`:skill <name> [args]` — run a skill now**, on the session in
    /// front (Steven, 10-06: "`:skill shannon-review` for example"): a turn
    /// whose prompt says to run it and carries its text as a `skill` context
    /// block, so the session records exactly what the model was given.
    /// Refused while anything runs there, as `:continue` is; asks the trust
    /// question first where a prompt would.
    pub(crate) fn skill_command(&mut self, args: &str) {
        let args = args.trim();
        let (name, rest) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
        if name.is_empty() {
            return self.refuse("skill: which? `:skill <name>` — `:skills` lists them");
        }
        // A skill runs as a turn in the session aimed at: with none
        // (`shannon tutor`, before one is called) it did nothing, unsaid
        // (review of 850a72d, L4).
        if self.aimed_session().is_none() {
            let why = self.no_agent();
            return self.refuse(format!("skill: {why}"));
        }
        let target = self.prompt_target();
        if self.refuse_if_busy("skill", target) {
            return;
        }
        let text = match self.skill_text(self.session_kind(target), name, rest) {
            Ok(text) => text,
            Err(why) => return self.refuse(format!("skill: {why}")),
        };
        // Claude Code in a directory not yet trusted asks first, as a prompt
        // would; the skill is not run until it is answered.
        self.ask_trust_up_front();
        if self.setup.is_some() {
            return self.notify(format!(
                "skill: answer the question first — then `:skill {args}` again"
            ));
        }
        let prompt = match rest.trim() {
            "" => format!("Run the `{name}` skill."),
            what => format!("Run the `{name}` skill: {what}"),
        };
        // Claude Code's own: the prompt alone, and `claude` loads it.
        let skill = text.map(|text| shannon_core::Block::Context {
            kind: shannon_core::ContextKind::Skill,
            source: name.to_string(),
            text,
        });
        self.start_turn_on(target, &prompt, skill.into_iter().collect());
    }

    /// **`:memories` / `:skills`** — the screen, open on that list, read
    /// now; not over a y/N waiting (`refuse_over_a_confirm`).
    pub(crate) fn shelf_command(&mut self, shelf: Shelf) {
        let verb = match shelf {
            Shelf::Memories => "memories",
            Shelf::Skills => "skills",
            Shelf::Servers => "mcp",
        };
        if self.refuse_over_a_confirm(verb) {
            return;
        }
        let (header, memories, skills) = match self.shelf_lists() {
            Ok(lists) => lists,
            Err(why) => return self.refuse(format!("{verb}: {why}")),
        };
        let mut screen = crate::shelf::Screen::new(shelf, header, memories, skills);
        screen.set_servers(self.servers_list());
        self.setup = Some(crate::setup::Setup::shelf(screen));
        self.refresh_shelf_preview(true);
        self.chat_dirty = true;
    }

    /// **The screen's lists, read from disk** — for the session in front,
    /// by its kind of model: its header, then `:memories`' and `:skills`'.
    fn shelf_lists(&self) -> Result<(String, List, List), String> {
        let buffer = self.prompt_target();
        let session = self.session_kind(buffer);
        let found = self
            .found(session)
            .ok_or("memory and skills are off: launch set no place to read them from")?;
        let id = self
            .session_of(buffer)
            .map(|b| self.session_tag(b.conversation.episode().id))
            .unwrap_or_default();
        let header = format!("Session {id} · {}", session_word(session));
        // Why `auto` is not what a write does — not ":messages says why"
        // where nothing was said: Claude Code's place waits on its first
        // turn here, silently (Reviewer A, round 28).
        let unvouched = match &found.write {
            Some(knowledge::Write::ClaudeCode(_)) if self.claude_memory_pending => {
                "Claude Code has not kept this project yet; from the end of its first turn here"
            }
            _ => ":messages says why",
        };
        let memories = memories_list(
            &found,
            self.write_mode(session),
            self.config.memory.mode,
            unvouched,
        );
        let skills = skills_list(&found, &self.config.skills.paths);
        Ok((header, memories, skills))
    }

    /// **The open screen read again** — after what can change it while it
    /// is open: its own keys (an edit, `x`, `d`, `n`), a turn's end (the
    /// model writes memories; the turn runs on behind the screen), and
    /// Claude Code naming its MCP servers. The screen is modal,
    /// so `:set`, `:model` or `:open` cannot run under it; it is built
    /// afresh each time it opens (review 28, Reviewer C: those three calls
    /// could not fire). None open: no disk is read.
    pub(crate) fn refresh_shelves(&mut self) {
        let open = self
            .setup
            .as_ref()
            .is_some_and(|s| s.entry() == crate::setup::Entry::Shelf);
        if !open {
            return;
        }
        let servers = self.servers_list();
        match self.shelf_lists() {
            Ok((header, memories, skills)) => {
                if let Some(screen) = self.setup.as_mut().and_then(|s| s.shelf_mut()) {
                    screen.refill(header, memories, skills);
                    screen.set_servers(servers);
                }
                // The model may have written the file it shows.
                self.refresh_shelf_preview(true);
            }
            Err(why) => self.say_on_shelf(why),
        }
    }

    /// **A key the float's list does not claim — the shared keymap's**
    /// (Steven, 10-07: "share primitives"). Its motion moves the row (`5j`,
    /// `gg`, `{n}G`), its scroll pages; a count or a prefix waits for the
    /// next key, as anywhere; what a list of rows has no use for is said on
    /// the screen, never dropped — and a Visual it would have started is
    /// left, so the mode cannot outlive the screen.
    pub(crate) fn navigate_shelf(&mut self, key: crossterm::event::KeyEvent) {
        use crate::keymap::{ScrollCmd, ScrollMagnitude};
        let outcome = self.chat_keymap.translate(key);
        let selecting = matches!(
            outcome,
            KeyOutcome::Edit {
                action: EditAction::SetAnchor
                    | EditAction::SetAnchorLine
                    | EditAction::SetAnchorBlock,
                ..
            }
        );
        if selecting {
            let _ = self.chat_keymap.leave_visual();
        }
        let Some(screen) = self.setup.as_mut().and_then(|s| s.shelf_mut()) else {
            return;
        };
        let rows = |m: ScrollMagnitude| match m {
            ScrollMagnitude::Line => 1,
            ScrollMagnitude::HalfPage => crate::shelf::Screen::PAGE / 2,
            ScrollMagnitude::Page => crate::shelf::Screen::PAGE,
        };
        let refused: Option<&str> = match outcome {
            KeyOutcome::Edit {
                action: EditAction::Move(motion),
                count,
            } => screen.move_rows(motion, count).err(),
            KeyOutcome::Scroll(ScrollCmd::Down(m)) => {
                screen.page_rows(true, rows(m));
                None
            }
            KeyOutcome::Scroll(ScrollCmd::Up(m)) => {
                screen.page_rows(false, rows(m));
                None
            }
            // A count, a `g`, an operator: waiting for the next key. And
            // `Esc` leaving a Visual: nothing to say.
            KeyOutcome::Ignored => None,
            KeyOutcome::Edit {
                action: EditAction::ClearAnchor,
                ..
            } => None,
            _ if selecting => Some("a list selects one row — the one under the cursor"),
            KeyOutcome::Refused(why) => Some(why),
            _ => Some("not on this list — q leaves it, then the key works"),
        };
        if let Some(why) = refused {
            screen.say(why);
        }
    }

    /// **The float's preview, read again** — the file under its cursor, only
    /// when the cursor has moved to another file, or `force`d when the file
    /// may have changed (an edit, a removal, a turn's end). Read bounded
    /// (`knowledge::peek`): for your screen, and never on the draw. An MCP
    /// server has no file to show; a file the model is not shown says why
    /// rather than what it holds.
    pub(crate) fn refresh_shelf_preview(&mut self, force: bool) {
        let Some(screen) = self.setup.as_mut().and_then(|s| s.shelf_mut()) else {
            return;
        };
        let Some(row) = screen.selected().cloned() else {
            screen.set_preview(None);
            return;
        };
        if !force && screen.preview_path() == Some(row.path.as_path()) {
            return;
        }
        let text = match (&row.mark, screen.shelf()) {
            (_, Shelf::Servers) => Err(format!(
                "`{}` is an MCP server Claude Code runs — no file of Shannon's to show; \
                 `claude mcp get {}` describes it",
                row.name, row.name
            )),
            (crate::shelf::Mark::NotRead(why), _) => Err(format!("not read — {why}")),
            _ => knowledge::peek(&row.path, SHELF_PREVIEW_CAP)
                .map_err(|why| format!("cannot show it — {why}")),
        };
        screen.set_preview(Some(crate::shelf::Preview {
            path: row.path,
            text,
        }));
    }

    /// The MCP list, from what Claude Code last said and what you disabled.
    fn servers_list(&self) -> List {
        servers_list(self.claude_servers.as_deref(), &self.config.mcp.disabled)
    }

    /// **`<Enter>` on the screen: `path` in `$EDITOR`** — the real file,
    /// edited in place (Steven, 10-06: "allowing them to be selected and
    /// opened in `$EDITOR`"); then the lists read again, the screen still
    /// open. What Shannon sends, the next turn tells again where it changed:
    /// what is told is read from the files and the history, so there is
    /// nothing else to update. A `listed` file is read when the model asks
    /// for it; a `claude's` one is claude's to read; a `not read` one, by no
    /// one (Reviewer A, round 28: this said every edit was told).
    /// `edit` is the seam a test drives (`ShannonTerminal` is a real stdout).
    ///
    /// It answers no [`Flow`](super::Flow): the pane it replaced returned
    /// `Ok(true)`, which the event loop read as QUIT, so every `<Enter>` —
    /// an edit, or a refusal — ended Shannon (Steven, 10-06).
    pub(crate) fn edit_shelf_file(&mut self, path: &Path, edit: impl FnOnce(&Path) -> Result<()>) {
        // Read again first: what is said is said on the screen as it is now.
        let said = edit(path);
        self.refresh_shelves();
        match said {
            Ok(()) => self.tell_on_shelf(format!(
                "edited {} — the next turn tells the model what changed",
                tilde(path)
            )),
            Err(e) => self.say_on_shelf(format!("edit: {e}")),
        }
        self.chat_dirty = true;
    }

    /// **`x` on the screen: disable a file, or enable it again** — for the
    /// session, through `:set` (the one door a setting changes by), so it is
    /// echoed, stacked and undone as any `:set` is; `[skills] disabled` /
    /// `[memory] disabled` in your config keeps it, since Shannon never
    /// writes your config (Steven, 10-06). A skill by its name — every skill
    /// of it — a memory or an instruction file by its path.
    pub(crate) fn toggle_shelf_file(&mut self, toggle: crate::shelf::Toggle) {
        use crate::shelf::Shelf;
        use shannon_core::config::set::Op;
        let table = match toggle.shelf {
            Shelf::Skills => "skills",
            Shelf::Memories => "memory",
            Shelf::Servers => "mcp",
        };
        let item = self.found_item(&toggle.path);
        // `claude` loads its own CLAUDE.md itself — `find` marks exactly
        // those `claude's` — and nothing Shannon refuses reaches that (review
        // 29, Reviewers A and C: `x` said "not told" while claude read it
        // every turn). Taking an entry back is still yours.
        if toggle.disable
            && let Some(i) = &item
            && i.role == Role::Instructions
            && i.state == State::ClaudeCode
        {
            return self.say_on_shelf(
                "claude loads its own CLAUDE.md itself — x cannot stop that; `[memory] claude = \
                 \"off\"` stops it, with the rest of Claude Code's files",
            );
        }
        // What `x` writes: the row's REAL name (the screen shows a cleaned
        // one), a memory by its path.
        let value = match (toggle.shelf, &item) {
            (Shelf::Memories, _) => tilde(&toggle.path),
            (Shelf::Skills, Some(i)) => i.name.clone(),
            _ => toggle.name.clone(),
        };
        let before = self.row_disabled_by(&toggle);
        match (toggle.disable, before) {
            (true, _) => {
                self.set_list(table, Op::Add, &value);
                let what = match toggle.shelf {
                    Shelf::Servers => "a call to any of its tools refused",
                    _ => "not told, not listed, a read of it refused",
                };
                if self.row_disabled_by(&toggle).is_some() {
                    self.tell_on_shelf(format!(
                        "`{value}` disabled for this session — {what}; `[{table}] disabled` in \
                         your config keeps it (`:config edit`)"
                    ));
                }
            }
            // Marked disabled, and nothing disables it now: said, and the
            // list read again below.
            (false, None) => self.say_on_shelf(format!("nothing disables `{value}` now")),
            // A place, or another spelling, holding more than this row: taking
            // it back is that entry's, not this row's (review 29: `x` said
            // "enabled" and nothing changed).
            (false, Some(why)) if !why.exact => self.say_on_shelf(format!(
                "disabled by `{}`, which holds more than this — `{}` enables all it holds",
                why.entry,
                take_back_line(table, &why.entry)
            )),
            (false, Some(why)) => {
                self.set_list(table, Op::Remove, &why.entry);
                match self.row_disabled_by(&toggle) {
                    // A file that disables it does again when it is read
                    // next — said, so "enabled" is not read as for good.
                    None => {
                        let back = self.files_holding(&format!("{table}.disabled"), &why.entry);
                        let again = match back.is_empty() {
                            true => String::new(),
                            false => format!(
                                " — {} disables it again at the next launch or `:reload`",
                                back.join(", ")
                            ),
                        };
                        self.tell_on_shelf(format!("`{value}` enabled for this session{again}"))
                    }
                    // Re-asked, not assumed: a `:set` refused, or an entry
                    // spelled another way, still holds it.
                    Some(still) => self.say_on_shelf(format!(
                        "still disabled, by `{}` — `:verbose set {table}.disabled?` says where \
                         it comes from",
                        still.entry
                    )),
                }
            }
        }
        self.refresh_shelves();
    }

    /// The config files whose own `key` list holds `entry` (`:verbose set`'s
    /// answer, `ConfigStack::setters`) — `~`-written.
    fn files_holding(&self, key: &str, entry: &str) -> Vec<String> {
        let held = |value: &toml::Value| {
            value
                .as_array()
                .is_some_and(|list| list.iter().any(|v| v.as_str() == Some(entry)))
        };
        self.config_stack
            .setters(key)
            .into_iter()
            .filter_map(|(_, path, value)| path.filter(|_| held(value)).map(tilde))
            .collect()
    }

    /// The file found at `path`, for the session in front.
    fn found_item(&self, path: &Path) -> Option<Item> {
        self.found(self.session_in_front())?
            .items
            .into_iter()
            .find(|i| i.path == path)
    }

    /// **What disables the row `toggle` is on, now** — the one predicate
    /// (`DisabledSet::why`), or, for a server, `[mcp] disabled` by its name.
    fn row_disabled_by(&self, toggle: &crate::shelf::Toggle) -> Option<knowledge::Why> {
        if toggle.shelf == crate::shelf::Shelf::Servers {
            return self
                .config
                .mcp
                .disabled
                .iter()
                .find(|d| shannon_core::McpServer::named_by(&toggle.name, d))
                // The entry as the list holds it: `-=` takes that spelling.
                .map(|d| knowledge::Why {
                    entry: d.clone(),
                    exact: true,
                });
        }
        let found = self.found(self.session_in_front())?;
        let item = found.items.iter().find(|i| i.path == toggle.path)?;
        let names = [
            item.name.as_str(),
            item.alias.as_deref().unwrap_or_default(),
        ];
        let real = std::fs::canonicalize(&item.path).unwrap_or_else(|_| item.path.clone());
        found.disabled.why(item.role, &names, &item.path, &real)
    }

    /// `:set <table>.disabled op [value]`, the value as ONE list entry —
    /// typed, never re-parsed: a `,` or a space split it into junk entries
    /// (review 29, Reviewers A, B, C).
    fn set_list(&mut self, table: &str, op: shannon_core::config::set::Op, value: &str) {
        let list = toml::Value::Array(vec![toml::Value::String(value.to_string())]);
        let item = shannon_core::config::set::Item::Assign {
            key: format!("{table}.disabled"),
            op,
            value: list.to_string(),
        };
        self.set_items(vec![item], false);
    }

    /// **`d` on the screen: remove a memory or a skill** — asked on the
    /// screen, saying what goes (`knowledge::removal`: a memory and its
    /// index lines, a skill's directory); answered `y`, worked out again
    /// from the file as it is now, and removed. A file not removable here
    /// says why on the screen. What the model was told stays in the
    /// history; the next turn says a told file is gone.
    pub(crate) fn remove_shelf_file(&mut self, removal: crate::shelf::Removal) {
        match (self.removal_plan(&removal.path), removal.confirmed) {
            (Err(why), _) => self.say_on_shelf(why),
            // Asked first; and asked AGAIN when `y` finds a plan other than
            // the one asked about — what goes is what you agreed to.
            (Ok((plan, name)), confirmed)
                if !confirmed || removal.asked.as_ref() != Some(&plan) =>
            {
                let question = match confirmed {
                    true => format!("it changed since you were asked — {}", plan.question(&name)),
                    false => plan.question(&name),
                };
                let asking = crate::shelf::Removal {
                    confirmed: false,
                    asked: Some(plan),
                    ..removal
                };
                if let Some(screen) = self.setup.as_mut().and_then(|s| s.shelf_mut()) {
                    screen.ask(question, asking);
                }
            }
            (Ok((plan, _)), _) => {
                let said = plan.apply();
                self.refresh_shelves();
                match said {
                    Ok(said) => self.tell_on_shelf(said),
                    Err(why) => self.say_on_shelf(why),
                }
            }
        }
    }

    /// Why a key on the screen did nothing ([`shelf_said`](Self::shelf_said)).
    fn say_on_shelf(&mut self, why: impl Into<String>) {
        self.shelf_said(NoticeKind::Refusal, why.into());
    }

    /// What a key on the screen did ([`shelf_said`](Self::shelf_said)).
    fn tell_on_shelf(&mut self, what: impl Into<String>) {
        self.shelf_said(NoticeKind::Report, what.into());
    }

    /// **What a key on the screen came to** — ON it, where the eyes are,
    /// while it is open, and in `:messages` always (`post`): a refusal said
    /// only on the screen was gone with it, an outcome said only on the
    /// message line sat behind it (review 29, Reviewer C: D2). Every shelf
    /// outcome goes through it, as `say_on_shelf` or `tell_on_shelf`.
    fn shelf_said(&mut self, kind: NoticeKind, text: String) {
        if let Some(screen) = self.setup.as_mut().and_then(|s| s.shelf_mut()) {
            match kind {
                NoticeKind::Refusal => screen.say(text.clone()),
                _ => screen.tell(text.clone()),
            }
        }
        self.post(kind, text);
    }

    /// **`n`, named: a new memory or skill from its template, in
    /// `$EDITOR`** — a memory where the session in front writes them
    /// (Claude Code's own directory in its session), a skill under
    /// `knowledge::skill_home`. Left as it was, it is taken away again, as
    /// everywhere Shannon opens `$EDITOR` for you (save-to-apply); a memory
    /// saved gets its line in the index beside it.
    pub(crate) fn create_shelf_file(
        &mut self,
        create: crate::shelf::Create,
        edit: impl FnOnce(&Path) -> Result<()>,
    ) {
        let draft = match self.draft_for(&create) {
            Ok(draft) => draft,
            Err(why) => return self.say_on_shelf(why),
        };
        if let Err(why) = draft.write() {
            return self.say_on_shelf(why);
        }
        // Settled whatever the editor did, and an editor that failed said
        // too: what it saved is kept (Steven, 10-07, with the reviewer — a
        // draft written is the user's), what it left as written is taken
        // away (review 29, M5: the outcome of a failed edit was dropped).
        let edited = edit(&draft.path);
        let said = draft.settle().map(|said| match &edited {
            Err(e) => format!("the editor failed ({e}) — {said}"),
            Ok(()) => said,
        });
        self.refresh_shelves();
        match said {
            Ok(said) => self.tell_on_shelf(said),
            Err(why) => self.say_on_shelf(why),
        }
    }

    /// The template `create` starts from, and where.
    fn draft_for(&self, create: &crate::shelf::Create) -> Result<knowledge::Draft, String> {
        match create.shelf {
            crate::shelf::Shelf::Memories => {
                knowledge::draft_memory(&self.memory_place()?, &create.name)
            }
            crate::shelf::Shelf::Skills => {
                let home = self.skill_place()?;
                // A second skill of a name: one of the two shadows the other
                // (review 29) — Enter edits the one there.
                let found = self.found(self.session_in_front());
                let there = knowledge::slug(&create.name).and_then(|slug| {
                    found?.items.into_iter().find(|i| {
                        i.role == Role::Skill
                            && (i.name == slug || i.alias.as_deref() == Some(slug.as_str()))
                    })
                });
                if let Some(there) = there {
                    return Err(format!(
                        "a skill `{}` is found already, at {} — Enter on it edits it",
                        there.name,
                        tilde(&there.path)
                    ));
                }
                knowledge::draft_skill(&home, &create.name)
            }
            crate::shelf::Shelf::Servers => Err(SERVERS_ARE_CLAUDES.into()),
        }
    }

    /// **Where a new memory goes** — the session in front's write place —
    /// or why there is none; `n` and `N` both ask.
    fn memory_place(&self) -> Result<PathBuf, String> {
        let found = self
            .found(self.session_in_front())
            .ok_or("memory and skills are off")?;
        found
            .write
            .map(|w| w.dir().to_path_buf())
            .ok_or_else(|| "no place to write memories — memory = off, or no `write`".into())
    }

    /// **Where a new skill goes** (`knowledge::skill_home`), or why there
    /// is none; `n` and `N` both ask.
    fn skill_place(&self) -> Result<PathBuf, String> {
        let roots = self
            .knowledge_roots
            .as_ref()
            .ok_or("memory and skills are off")?;
        knowledge::skill_home(&self.config.skills, roots)
            .ok_or_else(|| "no `[skills]` location is one place to make a skill in".into())
    }

    /// **`N`: the model writes it** (Steven, 10-06: "Good ideas on things
    /// like prompting the model") — the screen closed and a request drafted
    /// in the prompt for you to finish and send: a memory as the model is
    /// told to keep one, a skill where `n` would make it. Nothing is sent;
    /// a draft of yours in the prompt is never replaced.
    pub(crate) fn ask_model_to_write(&mut self, shelf: crate::shelf::Shelf) {
        if !self.prompt().text().trim().is_empty() {
            return self
                .say_on_shelf("the prompt holds a draft of yours — send or clear it, then N");
        }
        // Where it would go, or why nowhere — as `n` asks: a request for a
        // memory under `memory = off` was drafted and refused only when sent
        // (review 29).
        let text = match shelf {
            crate::shelf::Shelf::Servers => Err(SERVERS_ARE_CLAUDES.to_string()),
            crate::shelf::Shelf::Memories => self
                .memory_place()
                .map(|_| "Remember this, as a memory: ".to_string()),
            crate::shelf::Shelf::Skills => self.skill_place().map(|home| {
                format!(
                    "Write a new skill in {}/<name>/SKILL.md — frontmatter with its `name` and a \
                     `description` of when to use it, then its steps. The skill: ",
                    tilde(&home)
                )
            }),
        };
        let text = match text {
            Ok(text) => text,
            Err(why) => return self.say_on_shelf(why),
        };
        self.setup = None;
        self.prompt_mut().replace_with(&text);
        self.set_focus(PaneKind::Prompt);
        self.notify("drafted in the prompt — `A` finishes it, Enter sends it");
    }

    /// What removing the file at `path` takes, and its name — for the
    /// session in front, as the screen lists it.
    fn removal_plan(&self, path: &Path) -> Result<(knowledge::Removal, String), String> {
        let found = self
            .found(self.session_in_front())
            .ok_or("memory and skills are off")?;
        let roots = self
            .knowledge_roots
            .as_ref()
            .ok_or("memory and skills are off")?;
        let item = found
            .items
            .iter()
            .find(|i| i.path == path)
            .ok_or_else(|| format!("{} is not found now", tilde(path)))?;
        knowledge::removal(item, roots, &self.config.memory, &self.config.skills)
            .map(|plan| (plan, item.name.clone()))
    }

    /// **The model loads a skill** (the `skill` tool) — tracked under the
    /// work that asked, so its result reaches that session's round wherever
    /// it is; recorded as any call is; answered at once. No gate: it runs
    /// nothing, and reads only a file your config's locations found.
    pub(crate) fn answer_skill_call(
        &mut self,
        owner: GenerationId,
        call: &str,
        input: &serde_json::Value,
    ) {
        // The record names the buffer, so no path here leaves a tracked call
        // unanswered — an unanswered call holds its round open.
        let buffer = self.track_tool_call(call, Some(owner));
        let name = input["name"].as_str().unwrap_or_default().trim();
        let args = input["arguments"].as_str().unwrap_or_default();
        // Recorded as what happened: a name that is no skill said "loaded"
        // (Reviewer A, round 27). Claude Code's own never reaches here —
        // its session loads with its own tool — but if one did, it is said.
        let answer = self
            .skill_text(self.session_kind(buffer), name, args)
            .and_then(|text| {
                text.ok_or_else(|| format!("`{name}` is Claude Code's own skill: claude loads it"))
            });
        let (refused, detail) = match &answer {
            Ok(_) => (false, ""),
            Err(why) => (true, why.as_str()),
        };
        let verb = super::own_tool_verb(shannon_core::SKILL_TOOL, refused).unwrap_or("loaded");
        let shape = self.record_shape();
        let record = crate::tool_record::Record::call(verb, shannon_core::SKILL_TOOL, input, shape);
        let detail = detail.to_string();
        self.record_tool_activity(buffer, call, record, &detail);
        let (text, refused) = match answer {
            Ok(text) => (text, false),
            Err(why) => (why, true),
        };
        self.finish_tool_call(call, text, refused);
    }
}
