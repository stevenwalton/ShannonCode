//! **What the model is told now, and what a turn must carry of it** — the
//! text of what [`find`](super::find) found, and the difference from what the
//! history says was already sent.

use std::path::Path;

use super::{Found, Role, Session, State, read};
use crate::config::MemoryMode;
use crate::message::{Block, ContextKind, Message};

/// One thing the model is told: what it is, whose, and the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Told {
    pub kind: ContextKind,
    /// A path, or `shannon:memory` / `shannon:skills` for Shannon's own.
    pub source: String,
    pub text: String,
}

/// The most of an instruction file, a memory index, or a memory named
/// outright, sent: past any real one (Claude Code warns at 40k characters and
/// sends it all), short of what one wrong path could cost a turn.
const FILE_CAP: usize = 40 << 10;
/// The most of the skills list.
const SKILLS_CAP: usize = 8 << 10;
/// The most the list's "… and N more" line can take.
const MORE_ROOM: usize = 64;
/// The most of one skill's description in the list: 1,536 characters, where
/// Claude Code's skills documentation truncates a listing's (its
/// `description` with `when_to_use`).
const DESCRIPTION_CAP: usize = 1_536;
/// The most of a skill's text.
const SKILL_CAP: usize = 64 << 10;
/// **The most of all the files told, together** — each is bounded, their
/// number is not: a repository whose `AGENTS.md` is a directory of five
/// hundred sent 20 MB in a turn (Reviewer B, round 27). Past it, a file is
/// named and not read ([`UNSENT`]).
const TOTAL_CAP: usize = 128 << 10;
/// What a file past [`TOTAL_CAP`] is told as — the same every turn, so the
/// history does not send it again.
const UNSENT: &str = "[not sent: the files told passed 128 KB — read it if it is relevant]";

/// **What the model is told now**: each file sent whole, read bounded (an
/// unreadable or empty one is not told — the history then marks it
/// unloaded), until all of them pass `TOTAL_CAP`; Shannon's guidance on
/// memories when it may write them; the skills there are. The order a reader
/// meets them — instructions, memories, guidance, skills — is the order the
/// budget is spent in.
pub fn context(found: &Found, mode: MemoryMode, session: Session) -> Vec<Told> {
    let mut told = Vec::new();
    let mut spent = 0;
    for item in found.items.iter().filter(|i| i.state == State::Loaded) {
        let kind = match item.role {
            Role::Instructions => ContextKind::Instructions,
            Role::MemoryIndex | Role::Memory => ContextKind::Memory,
            Role::Skill => continue,
        };
        let source = item.path.display().to_string();
        if spent >= TOTAL_CAP {
            told.push(Told {
                kind,
                source,
                text: UNSENT.to_string(),
            });
            continue;
        }
        let Ok(read) = read::bounded(&item.path, FILE_CAP) else {
            continue;
        };
        if read.text.trim().is_empty() {
            continue;
        }
        // An index is told as it is, a line naming a memory you disabled
        // too: disabling is of files and places — the file is refused, the
        // index not edited for you (Steven, 10-07: "Just disable the
        // referenced file"). To silence an index, disable it, or its place.
        let text = match read.cut {
            true => format!(
                "{}\n[cut at {} KB — the rest is in the file]",
                read.text,
                FILE_CAP >> 10
            ),
            false => read.text,
        };
        spent += text.len();
        told.push(Told { kind, source, text });
    }
    // Shannon's own place only: in its session, Claude Code's prompt says
    // how to keep its own, and saying it again named a second place.
    if let Some(super::Write::Shannon(write)) = &found.write {
        told.push(Told {
            kind: ContextKind::Instructions,
            source: "shannon:memory".into(),
            text: guidance(write, mode),
        });
    }
    if let Some(list) = skills_list(found, session) {
        told.push(Told {
            kind: ContextKind::Skills,
            source: "shannon:skills".into(),
            text: list,
        });
    }
    told
}

/// **How the model keeps memories**, where it may write them — in the format
/// Claude Code's own memory files have (one file a memory, frontmatter, a
/// `MEMORY.md` index), by rules that practice taught: no activity logs, no
/// duplicates, absolute dates, and a memory naming a file is a claim to check
/// (`docs/DesignDocs/memory-and-skills.md`).
fn guidance(write: &Path, mode: MemoryMode) -> String {
    let dir = write.display();
    let asks = match mode {
        MemoryMode::Ask => "Each write there asks the user first.",
        MemoryMode::Auto | MemoryMode::Off => "Writes there run without asking.",
    };
    format!(
        "You keep memories across sessions in {dir}: one file per memory, and a line for each in \
         {dir}/MEMORY.md — `- [Title](file.md) — a one-line hook`. A memory file begins with \
         frontmatter (`name`, `description`, `type`: user, feedback, project or reference), then \
         the fact; a feedback or project memory follows it with **Why:** and **How to apply:** \
         lines. {asks}\n\
         Save one when the user asks you to remember something, or when you learn what a later \
         session will need: who the user is, how they want you to work, a decision and its \
         reason. Do not save what the code, the history or an instruction file already holds, or \
         what matters only now. Update a memory rather than add a second; delete one the user \
         asks you to forget, and fix one that proves wrong. Write dates in full. A memory that \
         names a file, a function or a flag is a claim to check before you rely on it.\n\
         The `memory` context above holds each memory location's index; read a memory's file \
         when its line is relevant."
    )
}

/// The skills Shannon names to the model, by name and description — `None`
/// when there are none. Claude Code's own skills are not named in a Claude
/// Code session: `claude` lists its own.
fn skills_list(found: &Found, session: Session) -> Option<String> {
    let listed: Vec<_> = found
        .skills()
        .filter(|item| item.state == State::Listed)
        .collect();
    if listed.is_empty() {
        return None;
    }
    let mut out = match session {
        Session::Api => {
            "Skills you can load with the `skill` tool, by name, when one fits the task:\n"
                .to_string()
        }
        Session::ClaudeCode => {
            "Skills Shannon found — read one's file when it fits the task:\n".to_string()
        }
    };
    for (i, item) in listed.iter().enumerate() {
        let mut said: String = item
            .description
            .as_deref()
            .unwrap_or("(no description)")
            .chars()
            .take(DESCRIPTION_CAP)
            .collect();
        if session == Session::ClaudeCode {
            said.push_str(&format!(" ({})", item.path.display()));
        }
        let line = format!("- {}: {said}\n", item.name);
        // Room kept for the line that says so, which counts too.
        if out.len() + line.len() + MORE_ROOM > SKILLS_CAP {
            out.push_str(&format!(
                "… and {} more (`:skills` lists them)\n",
                listed.len() - i
            ));
            break;
        }
        out.push_str(&line);
    }
    Some(out.trim_end().to_string())
}

/// **What a turn must carry**: each [`Told`] the history does not already
/// hold as it is now — new, or changed — and, for a source the history
/// holds that is told no longer, an `unloaded` block. What is loaded is read
/// from the history alone (the latest block per source; an `unloaded` one
/// ends it), so a composed turn that dropped its blocks (`S`, `:return`, a
/// prefill `:continue`) just has them sent again, and a resumed session or a
/// branch knows what its own history told. A skill's text is a one-off, never
/// compared. A source in `kept` is never unloaded: the model has it from
/// elsewhere — Claude Code reads its own CLAUDE.md — and a session switched
/// to Claude Code was told its CLAUDE.md was gone while `claude` read it
/// (Reviewer A, round 27).
pub fn attach(history: &[Message], now: &[Told], kept: &[String]) -> Vec<Block> {
    let mut sent: Vec<(&str, ContextKind, &str)> = Vec::new();
    for block in history.iter().flat_map(|m| &m.blocks) {
        let Block::Context { kind, source, text } = block else {
            continue;
        };
        match kind {
            ContextKind::Skill => {}
            ContextKind::Unloaded => sent.retain(|(s, _, _)| s != source),
            kind => {
                sent.retain(|(s, _, _)| s != source);
                sent.push((source, *kind, text));
            }
        }
    }
    let mut out: Vec<Block> = now
        .iter()
        .filter(|t| {
            !sent
                .iter()
                .any(|(s, k, x)| *s == t.source && *k == t.kind && *x == t.text)
        })
        .map(|t| Block::Context {
            kind: t.kind,
            source: t.source.clone(),
            text: t.text.clone(),
        })
        .collect();
    out.extend(
        sent.iter()
            .filter(|(s, _, _)| !now.iter().any(|t| t.source == *s) && !kept.iter().any(|k| k == s))
            .map(|(s, _, _)| Block::Context {
                kind: ContextKind::Unloaded,
                source: s.to_string(),
                text: "No longer loaded: do not rely on what it said.".into(),
            }),
    );
    out
}

/// **A skill's text, as it is invoked**: its frontmatter dropped, where it
/// lives said first (its supporting files are named from there, as Claude
/// Code's are), `$ARGUMENTS` filled — or the arguments added after, when the
/// text does not place them. Never from a path `sensitive` names, as written
/// or as it resolves (`never_read`). At most `SKILL_CAP` whatever the
/// arguments: each `$ARGUMENTS` is a copy of them, and a model's arguments
/// in a text that names them often multiplied the cap (Reviewer B, round 27:
/// 61 MB from a 66 KB skill).
pub fn skill_text(
    path: &Path,
    args: &str,
    sensitive: &dyn Fn(&Path) -> Option<String>,
) -> Result<String, String> {
    if let Some(why) = super::never_read(path, &super::resolved(path), sensitive) {
        return Err(format!("{} is {why}", path.display()));
    }
    let read = read::bounded(path, SKILL_CAP)?;
    let (_, body) = read::frontmatter(&read.text);
    let args = args.trim();
    // The BODY places them or not: a frontmatter that mentions `$ARGUMENTS`
    // (a description saying how to call it) placed nothing (Reviewer C).
    let placed = body.contains("$ARGUMENTS");
    // Built a piece at a time and stopped at the cap: replacing first and
    // cutting after bounded what was SENT, not the work — a body of 6k
    // `$ARGUMENTS` and 100 KB of arguments was 650 MB built on the thread
    // that draws (Reviewer E1, round 27).
    let mut out = String::new();
    let mut long = false;
    let tail = (!args.is_empty() && !placed).then(|| format!("\n\nARGUMENTS: {args}"));
    let pieces = body.trim().split("$ARGUMENTS").enumerate();
    for piece in pieces
        .flat_map(|(i, piece)| [(i > 0).then_some(args), Some(piece)])
        .flatten()
        .chain(tail.as_deref())
    {
        out.push_str(piece);
        if out.len() > SKILL_CAP {
            let mut end = SKILL_CAP;
            while !out.is_char_boundary(end) {
                end -= 1;
            }
            out.truncate(end);
            long = true;
            break;
        }
    }
    let mut body = out;
    if read.cut || long {
        body.push_str(&format!(
            "\n[cut at {} KB — the rest is in the file]",
            SKILL_CAP >> 10
        ));
    }
    let dir = path.parent().unwrap_or(path).display();
    Ok(format!("Base directory for this skill: {dir}\n\n{body}"))
}
