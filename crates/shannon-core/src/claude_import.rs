//! **Reading a Claude Code conversation** — the file `claude` keeps for each
//! session, for `:graph` to list and `:import` to bring in as a Shannon
//! session (`docs/DesignDocs/claude-code-backend.md`, "Claude Code
//! conversations in `:graph`").
//!
//! The format is Claude Code's INTERNAL one (`~/.claude/projects/<dir>/
//! <id>.jsonl`), not an API: what is known is read, what is not is skipped and
//! counted, never guessed at. The shapes were measured on Claude Code's own
//! files (2.1.288, by a probe of their structure alone — keys, counts,
//! booleans, never text; Steven, 10-04).
//!
//! - Entries chain by `uuid`/`parentUuid`; the conversation is the path back
//!   from the newest entry. A compaction's boundary has no `parentUuid` — a
//!   resume stops there, at the summary that follows it — and the older
//!   history goes on behind it: named by `logicalParentUuid` in older files,
//!   and where the file stood when it compacted in newer ones (`link_back`;
//!   both seen in Steven's files).
//!   A conversation `claude --resume` made begins at a compaction copied from
//!   the file it was resumed from, which holds what came before.
//! - One reply is several entries: each content block is written alone, the
//!   blocks of one API message sharing a `message.id`. Parallel tool calls make
//!   the chain a DAG — each result parented on its own one-block entry — so a
//!   single-parent walk keeps one branch; the rest are recovered, so every
//!   parallel result is read.
//! - A file reaches hundreds of MB, so it is read twice, streamed: once for
//!   the chain (no bodies kept), once for the entries on it.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::message::Block;

/// Claude Code's own home: `$CLAUDE_CONFIG_DIR`, else `~/.claude` (its
/// settings documentation: the variable moves what it keeps there).
pub fn claude_home() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))
}

/// Past this many characters Claude Code cuts a project directory's name and
/// appends a hash (the Agent SDK's sessions documentation) — a hash it does
/// not give, so a name past it is one Shannon cannot know (Steven, 11-07:
/// such paths need no support). Taken from the docs on 11-07; until then a
/// longer name (to the filesystem's 255) was taken whole, and named a memory
/// directory Claude Code never makes.
const CUT_PAST: usize = 200;

/// The most of a `.git` file, `commondir` or `gitdir` read — each one path.
const GITFILE_CAP: usize = 4096;

/// **The directory Claude Code keeps `cwd`'s conversations in**, under
/// `projects`, if there is one — named as `directory_name` says, and none
/// for a path its documentation does not settle.
pub fn project_dir(projects: &Path, cwd: &Path) -> Option<PathBuf> {
    let dir = projects.join(directory_name(cwd)?);
    dir.is_dir().then_some(dir)
}

/// **The name Claude Code gives a project's directory, where its
/// documentation settles it**: the path with every character other than a
/// letter or digit replaced by `-` (its `.claude` directory reference:
/// `-Users-you-my-project`). That settles an ASCII path only — whether `é`
/// is a letter, and how many `-` an emoji leaves, it does not say — and a
/// name past [`CUT_PAST`] is cut and hashed as it does not say. `None` for
/// both: a name
/// guessed there finds another project's directory or none, and this one
/// decides where memories are read and written unasked. (Until 11-07 Shannon
/// assumed answers the docs do not give; it now assumes none.)
fn directory_name(path: &Path) -> Option<String> {
    let path = path.to_str().filter(|p| p.is_ascii())?;
    let name: String = path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    (name.len() <= CUT_PAST).then_some(name)
}

/// **Claude Code's own memory directory for a session started at `start`**
/// — `<home>/projects/<its repository's root, named>/memory`: per
/// repository, shared across its worktrees (its memory documentation), so
/// every worktree and subdirectory shares one; `start` itself outside a
/// repository. `None` where `directory_name` is. Not followed: its
/// `autoMemoryDirectory` setting — Shannon does not read Claude Code's
/// settings.
pub fn auto_memory_dir(home: &Path, start: &Path) -> Option<PathBuf> {
    Some(home.join("projects").join(root_name(start)?).join("memory"))
}

/// **`{project}` named as Claude Code names a project** (`[memory] project =
/// "claude-code"`): its directory's name under `projects/` — one for a
/// repository, its subdirectories and its worktrees. Where Claude Code's
/// name is not known (`directory_name`), Shannon's own name for the
/// directory (`store::project_name`): what this names is Shannon's directory,
/// so it needs a name, not Claude Code's.
pub fn project_key(start: &Path) -> String {
    root_name(start).unwrap_or_else(|| crate::store::project_name(start))
}

/// The name Claude Code gives the project `start` is in: its repository's
/// root's (`start` itself outside one), where [`directory_name`] knows it.
fn root_name(start: &Path) -> Option<String> {
    directory_name(&repository_root(start).unwrap_or_else(|| start.to_path_buf()))
}

/// **The repository a worktree belongs to**, whose memory its worktrees
/// share (Claude Code's memory documentation): the nearest `.git` above `start`,
/// and when that is a worktree's file, its main repository — only when the
/// layout is what `git worktree add` makes (git's own repository-layout
/// documentation, `gitrepository-layout(5)`: the worktree's git directory a
/// child of `<common>/worktrees/`, and its `gitdir` pointing back), since a
/// cloned repository writes its own `.git` file and could otherwise name any
/// project's memory. A bare repository's worktree: the common directory.
///
/// Its three files are read **bounded, regular files only**
/// (`knowledge::read::bounded`): this runs at every launch, after the
/// terminal is raw, on files anyone can place — a FIFO named `.git` in
/// `/tmp` hung Shannon on a blank screen Ctrl-C could not leave, and a `.git`
/// linked to `/dev/zero` read without end (Reviewer B, round 28). A cut
/// read is checked as any other — the layout checks below are the gate, and
/// a path cut short names no worktree they accept.
///
/// The back-link is resolved against the process's directory, so a
/// relative one (git's `worktree.useRelativePaths`) matches nothing, and the
/// worktree's own directory is the answer.
fn repository_root(start: &Path) -> Option<PathBuf> {
    let root = start.ancestors().find(|dir| dir.join(".git").exists())?;
    let read = |path: PathBuf| {
        crate::knowledge::read::bounded(&path, GITFILE_CAP)
            .ok()
            .map(|t| t.text)
    };
    let main = || -> Option<PathBuf> {
        let pointer = read(root.join(".git"))?;
        let gitdir = root.join(pointer.trim().strip_prefix("gitdir:")?.trim());
        let gitdir = std::fs::canonicalize(gitdir).ok()?;
        let common =
            std::fs::canonicalize(gitdir.join(read(gitdir.join("commondir"))?.trim())).ok()?;
        if gitdir.parent()? != common.join("worktrees") {
            return None;
        }
        let back = std::fs::canonicalize(read(gitdir.join("gitdir"))?.trim()).ok()?;
        if back != std::fs::canonicalize(root).ok()?.join(".git") {
            return None;
        }
        Some(match common.file_name().is_some_and(|n| n == ".git") {
            true => common.parent()?.to_path_buf(),
            false => common,
        })
    };
    Some(main().unwrap_or_else(|| root.to_path_buf()))
}

/// One conversation file, as a listing shows it — read without its body.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// The session id: the file's name.
    pub id: String,
    pub path: PathBuf,
    /// Its title, from the entries the file holds (measured): one the user
    /// set, then one generated, a summary, or the last prompt. The first
    /// three in the order Claude Code's session picker documents (its
    /// sessions docs, "Use the session picker"); where the picker falls back
    /// to the first prompt, Shannon takes the last, which the file's tail
    /// holds.
    pub title: Option<String>,
    pub modified: Option<std::time::SystemTime>,
    pub bytes: u64,
    /// **The conversations a `claude --resume` of this one began** — each a
    /// new file, named by the `continued-in` record Claude Code leaves in
    /// THIS one (the new file names nothing back), read from the same tail
    /// as the title. Ids only, compared with other listed conversations',
    /// never made a path.
    pub resumed_in: Vec<String>,
}

/// **The conversations in `dir`**, newest first. A directory that is not
/// there is no conversations; one that cannot be read says why.
pub fn list(dir: &Path) -> Result<Vec<Found>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{} could not be read ({e})", dir.display())),
    };
    let mut found: Vec<Found> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
        .filter_map(|path| {
            // The file itself, never what a link names: Claude Code's are
            // its own files, and a link is read as none (`open`).
            let meta = std::fs::symlink_metadata(&path)
                .ok()
                .filter(|m| m.is_file())?;
            let id = path.file_stem()?.to_str()?.to_string();
            let Tail { title, resumed_in } = tail_of(&path);
            Some(Found {
                title,
                id,
                modified: meta.modified().ok(),
                bytes: meta.len(),
                path,
                resumed_in,
            })
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.modified));
    Ok(found)
}

/// How much of a file's end a title is looked for in — titles are rewritten
/// as a conversation goes, so the newest is near the end.
const TITLE_TAIL: u64 = 256 * 1024;

/// **How many resumes a conversation's listing names, at most** — a real one
/// names a handful; a crafted tail could name thousands (review stop 5, B 4).
const RESUMES_KEPT: usize = 64;

/// What a listing reads of a conversation's end ([`tail_of`]).
#[derive(Debug, Default, PartialEq)]
struct Tail {
    title: Option<String>,
    resumed_in: Vec<String>,
}

/// What Claude Code calls the conversation in `path`, and the resumes it
/// began ([`Found::resumed_in`]), from its last [`TITLE_TAIL`] bytes — where
/// both are written: a title is rewritten as the conversation goes, and a
/// resume's record is appended as the conversation moves to its new file. A
/// resume recorded further back than that (the file grown past it since) is
/// not seen.
fn tail_of(path: &Path) -> Tail {
    let read = || -> Option<(String, bool)> {
        let mut file = open(path).ok()?;
        let len = file.metadata().ok()?.len();
        let start = len.saturating_sub(TITLE_TAIL);
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut tail = Vec::new();
        file.take(TITLE_TAIL).read_to_end(&mut tail).ok()?;
        Some((String::from_utf8_lossy(&tail).into_owned(), start > 0))
    };
    let Some((text, cut)) = read() else {
        return Tail::default();
    };
    let mut lines = text.lines();
    if cut {
        lines.next(); // a line cut in two
    }
    let mut best: (u8, Option<String>) = (0, None);
    let mut resumed_in = Vec::new();
    // Seen by a set, and at most `RESUMES_KEPT`: a crafted tail of three
    // thousand distinct records made the check O(k²) on the UI thread at
    // every listing (review stop 5, B 4). A real file names a handful.
    let mut seen = std::collections::HashSet::new();
    for line in lines {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let (rank, key) = match value["type"].as_str() {
            Some("custom-title") => (4, "customTitle"),
            Some("ai-title") => (3, "aiTitle"),
            Some("summary") => (2, "summary"),
            Some("last-prompt") => (1, "lastPrompt"),
            Some("continued-in") => {
                if let Some(id) = value["continuedInSessionId"].as_str()
                    && resumed_in.len() < RESUMES_KEPT
                    && seen.insert(id.to_string())
                {
                    resumed_in.push(id.to_string());
                }
                continue;
            }
            _ => continue,
        };
        if let Some(title) = value[key].as_str().filter(|t| !t.trim().is_empty())
            && rank >= best.0
        {
            best = (rank, Some(title.to_string()));
        }
    }
    // On one line, and whole: a reader scrubs a title before it cuts one
    // ([`one_line`]) — cut first, a credential across the cut kept its start
    // (`… sk-ant-ap…`), past any matcher (review stop 4, E 4).
    Tail {
        title: best
            .1
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")),
        resumed_in,
    }
}

/// **A result within [`RESULT_CAP`]**, cut on a character boundary and said
/// to be — for a result once it is scrubbed. A reading holds results whole:
/// cut first, a credential across the cut kept its start in the store and on
/// the wire, past any matcher (review stop 4, E 4).
pub fn cut_result(content: String) -> String {
    if content.len() <= RESULT_CAP {
        return content;
    }
    let mut end = RESULT_CAP;
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[… {} more bytes, cut on import]",
        &content[..end],
        content.len() - end
    )
}

/// `text` on one line, at most `max` characters — for a title once it is
/// scrubbed.
pub fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &flat[..at]),
        None => flat,
    }
}

/// How much of a conversation to bring in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// What `claude --resume` holds: from the latest compaction — its
    /// summary, the messages it kept, then what followed.
    Resume,
    /// Back to the first prompt, across every compaction.
    Everything,
    /// [`Everything`](Self::Everything), each compaction's summary KEPT as
    /// the turn it opened — the conversation as `:graph` shows it, its
    /// compactions where they happened. And the reading an import is found
    /// in: an import's first turn is a summary of the compaction that was
    /// latest THEN, which `Resume` no longer reaches once Claude Code has
    /// compacted again, and `Everything` leaves out.
    Lineage,
}

/// A conversation, read: its turns, and what was not brought in.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    /// The session id.
    pub id: String,
    /// The directory it last ran in.
    pub cwd: Option<String>,
    pub turns: Vec<ImportedTurn>,
    pub skipped: Skipped,
    /// It goes back further than was read, and why.
    pub earlier: Option<Earlier>,
    /// The newest entry on the conversation's path as it was read — after
    /// which a copy of this reading's last turn ends ([`Until::After`]).
    pub leaf: Option<String>,
}

/// **Where a copy ends** ([`read_until`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Until {
    /// Before the entry that opens a turn ([`ImportedTurn::uuid`]): the turn
    /// before it is the copy's last.
    Before(String),
    /// After an entry — a reading's [`leaf`](Imported::leaf), so a copy of
    /// its last turn holds what was shown, not what came since: on the last
    /// turn of a reading the file had outgrown, `<CR>` copied every turn
    /// after it, and said nothing of it (review stop 4, A 3).
    After(String),
}

/// Why a reading begins after the conversation's start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Earlier {
    /// [`Scope::Resume`] stopped at a compaction; the other scopes read on.
    Compacted,
    /// What came before a compaction is in another file: the conversation
    /// was resumed from one, and `claude --resume` copies the chain on into
    /// a new file from that compaction (Steven's files, 10-17).
    Elsewhere,
}

/// One turn: what the user said, the tool rounds the reply took, and the
/// reply's closing text — the order a live turn is built in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportedTurn {
    pub prompt: String,
    pub rounds: Vec<Round>,
    pub reply: String,
    /// The model that answered, as Claude Code recorded it.
    pub model: Option<String>,
    /// The entry that opened it — its prompt's, or the compaction summary's
    /// — as Claude Code names entries. Which entry opens a turn depends on
    /// the reading: [`Scope::Everything`] drops a summary, so the turn after
    /// a compaction is opened by another entry than under
    /// [`Scope::Lineage`] or [`Scope::Resume`] (and a reply straight after
    /// one joins the turn before). `None` for a turn whose prompt was not
    /// read (its prompt then says it was earlier, not imported).
    pub uuid: Option<String>,
    /// The entries whose prompts JOINED this one's before any reply — a
    /// summary's turn takes the question after it. Another reading can have
    /// a reply between, and the question opens a turn of its own there:
    /// [`Scope::Resume`] answers a summary with the messages its compaction
    /// kept, which [`Scope::Lineage`] reads before it.
    pub joined: Vec<String>,
    /// A compaction's summary opened it: where Claude Code compacted.
    pub summary: bool,
}

impl ImportedTurn {
    /// **Is `uuid` this turn**, in whichever reading named it — the entry
    /// that opened it here, or one that joined it here and opened its own
    /// turn in another ([`joined`](Self::joined)).
    pub fn holds(&self, uuid: &str) -> bool {
        self.uuid.as_deref() == Some(uuid) || self.joined.iter().any(|j| j == uuid)
    }
}

/// One tool round: the text the assistant wrote before its calls, the calls
/// (`Block::ToolUse`), and their results (`Block::ToolResult`), every call
/// answered.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Round {
    pub text: String,
    pub calls: Vec<Block>,
    pub results: Vec<Block>,
}

/// What was read and not brought in, by kind — said, never silently dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Skipped {
    /// The model's thinking.
    pub thinking: usize,
    /// Images, each left as a line saying so.
    pub images: usize,
    /// Claude Code's own messages to its model (`isMeta`).
    pub meta: usize,
    /// Attachments: files, hook output and reminders Claude Code added.
    pub attachments: usize,
    /// Claude Code's system entries (compaction boundaries, durations).
    pub system: usize,
    /// Shannon's own handoff — the history it seeded a session with, which
    /// it holds already, as turns.
    pub handoffs: usize,
    /// Compaction summaries, where the history they summarise was read.
    pub summaries: usize,
    /// Calls with no result recorded, each answered as such.
    pub unanswered: usize,
    /// Results past [`RESULT_CAP`]: held whole by the reading, cut once
    /// scrubbed ([`cut_result`]).
    pub cut: usize,
    /// Shapes this reader does not know.
    pub unknown: usize,
    /// Lines that are not JSON — and a line past the longest read.
    pub unparsable: usize,
}

/// The most of one tool result brought in: what Shannon keeps of a command's
/// output (`config::DEFAULT_EXEC_MAX_OUTPUT`).
pub const RESULT_CAP: usize = crate::config::DEFAULT_EXEC_MAX_OUTPUT;

/// What a call with no recorded result is answered with — the API refuses a
/// `tool_use` with none.
const UNANSWERED: &str = "[no result was recorded in the Claude Code transcript]";

/// The prompt of a turn whose prompt was not read (a reply first on the path).
const EARLIER: &str = "[earlier in the Claude Code conversation, not imported]";

/// What an image is left as.
const IMAGE: &str = "[an image, not imported]";

/// An entry on the chain, without its body.
#[derive(Debug, Clone)]
struct Node {
    kind: Kind,
    link: Link,
    message_id: Option<String>,
    /// A user entry carrying a tool result.
    tool_result: bool,
    /// A compaction's summary: the turn it opens begins at its boundary.
    summary: bool,
    timestamp: String,
    offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    User,
    Assistant,
    Attachment,
    System,
}

/// **Where the walk back goes from an entry.**
#[derive(Debug, Clone)]
enum Link {
    /// To its parent.
    Parent(String),
    /// Across a compaction, to the entry the history before it ends at —
    /// `None` when that history is in another file ([`Earlier::Elsewhere`]).
    Compaction(Option<String>),
    /// Nowhere: the conversation's first entry.
    Start,
}

/// **The link of an entry with no parent.** Older Claude Code files name the
/// history before a compaction: the boundary's `logicalParentUuid` is the
/// last entry before it, read already (observed in Steven's files). Later versions
/// keep the compaction's last messages (`compactMetadata.preservedSegment`)
/// and name the entry AFTER the summary instead — followed, that loops back
/// onto the walk, and a reading stopped at the latest compaction (Steven's
/// files, 10-17: one compaction read of 29, of 47). There the history ends
/// where the file stood when Claude Code compacted: its newest entry not a
/// subagent's, as the leaf is chosen. Measured on his six files, every
/// compaction is crossed so, back to the first prompt or to the compaction a
/// resume copied on — which has nothing read before it.
fn link_back(value: &Value, nodes: &HashMap<String, Node>, newest: Option<&str>) -> Link {
    let logical = value["logicalParentUuid"].as_str();
    if let Some(older) = logical.filter(|l| nodes.contains_key(*l)) {
        return Link::Compaction(Some(older.to_string()));
    }
    if logical.is_some() || value["subtype"] == "compact_boundary" {
        return Link::Compaction(newest.map(str::to_string));
    }
    Link::Start
}

/// **The messages a compaction kept, where they still precede it** — `before`
/// the walk up to the boundary, `head` the first it kept
/// (`compactMetadata.preservedSegment.headUuid`): from there to where the
/// history before it ends. Measured on Steven's files (10-17, structure
/// only), the head is an assistant message — the summary is the user's —
/// and a file `claude --resume` wrote holds the kept messages AFTER the
/// summary already, so finds none here. A compaction that kept none names
/// no head.
fn kept_before(before: &[String], head: Option<&String>) -> Vec<String> {
    head.and_then(|head| before.iter().position(|uuid| uuid == head))
        .map(|h| before[h..].to_vec())
        .unwrap_or_default()
}

/// **Read the conversation in `path`**, as much of it as `scope` asks.
pub fn read(path: &Path, scope: Scope) -> Result<Imported, String> {
    read_until(path, scope, None)
}

/// **[`read`], ending where `until` says** — before the turn an entry opens
/// (the one [`ImportedTurn::uuid`] names it by, as [`Scope::Lineage`] read
/// it), or after a reading's leaf — so what comes in is the conversation as
/// it stood there: `Resume` from the compaction latest THEN (Steven, 10-17:
/// `<CR>` on a turn "should import up to that turn"). A turn a compaction
/// opens begins at its boundary, so the compaction goes with it. `None`
/// reads to the end; an entry the conversation no longer holds is an error.
pub fn read_until(path: &Path, scope: Scope, until: Option<&Until>) -> Result<Imported, String> {
    let mut transcript = Transcript::new(path);
    transcript.read_on()?;
    transcript.take_unended()?;
    transcript.scoped(scope, until)
}

/// **A transcript read so far, to read on from where it stopped** (Steven,
/// 10-17: "Once we've imported conversations we shouldn't need to
/// re-read"). Claude Code only appends to a conversation's file, so what was
/// read stays true: a grown file costs its new lines, and its lineage is
/// built on from where its last turn opened ([`lineage`](Self::lineage)).
/// The live session's file grows with every message, and `:graph` read all
/// of it, every time (1.4 s of a 260 MB file in release, measured 10-17). A
/// file that no longer begins as what was read — rewritten, cut short — is
/// read from its start again.
#[derive(Debug)]
pub struct Transcript {
    path: PathBuf,
    heads: Heads,
    /// Where the last read stopped: the end of the last whole line.
    read_to: u64,
    /// That line's last bytes, at most [`SEAL`] — to tell a file appended to
    /// from one rewritten.
    seal: Vec<u8>,
    /// The lineage last built, to build on.
    built: Option<Built>,
}

/// How much of the last line read is kept to recognise the file by.
const SEAL: usize = 4096;

/// **The longest line read**: past it, a line is passed over to its end,
/// unread, and counted as not JSON. A newline-free file of gigabytes in this
/// project's directory was read whole into memory at the first `:graph`
/// (review stop 4, E 6). Claude Code's longest lines carry an image as
/// base64 — megabytes; this is well past them.
const LINE_CAP: usize = 64 * 1024 * 1024;

/// One line of `reader` read into `line`: how many bytes it spans, whether a
/// newline ended it, and whether it is in `line` whole — a line past `cap`
/// ([`LINE_CAP`]) is read over, only its last [`SEAL`] bytes kept (the file is
/// still known by them, `Transcript::goes_on`).
fn read_line(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
    cap: usize,
) -> std::io::Result<LineRead> {
    line.clear();
    let n = reader
        .by_ref()
        .take(cap as u64 + 1)
        .read_until(b'\n', line)?;
    if n <= cap || line.last() == Some(&b'\n') {
        return Ok(LineRead {
            spans: n,
            ended: line.last() == Some(&b'\n'),
            whole: true,
        });
    }
    let mut spans = n;
    let ended = loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            break false;
        }
        let (take, ended) = match buf.iter().position(|&b| b == b'\n') {
            Some(at) => (at + 1, true),
            None => (buf.len(), false),
        };
        line.extend_from_slice(&buf[..take]);
        let over = line.len().saturating_sub(SEAL);
        line.drain(..over);
        reader.consume(take);
        spans += take;
        if ended {
            break true;
        }
    };
    Ok(LineRead {
        spans,
        ended,
        whole: false,
    })
}

/// What [`read_line`] read.
struct LineRead {
    spans: usize,
    ended: bool,
    whole: bool,
}

/// **What pass 1 keeps of a transcript: each entry's head**, by uuid —
/// enough to walk the chain without a body — taken in the file's order,
/// one whole line at a time.
#[derive(Debug, Default)]
struct Heads {
    nodes: HashMap<String, Node>,
    /// Each compaction boundary that kept messages, and the first it kept.
    kept_from: HashMap<String, String>,
    /// The newest entry read so far that is not a subagent's: the leaf, once
    /// the file is read, and where the history before a compaction ends.
    newest: Option<String>,
    cwd: Option<String>,
    unparsable: usize,
    /// An entry was read again under a uuid read before — the last line
    /// wins — since the lineage was last built.
    restated: bool,
}

/// A lineage built, and where the next one can build on from.
#[derive(Debug)]
struct Built {
    imported: Imported,
    /// The chain it was built from.
    chain: Vec<String>,
    /// Where its last turn opened.
    last: Option<Opened>,
}

/// **Where a turn opened, and the build just after** — the entry's place in
/// the chain, the turns closed by then, the turn as it opened, and what had
/// been skipped. Everything after it is what a grown file can change; a
/// build goes on from the entry after. Taken AFTER the opening entry, not
/// before: that entry's own skipped images are counted before it closes the
/// turn before it, whose unanswered calls are counted then, and no count
/// taken before it could hold the one without the other.
#[derive(Debug, Clone)]
struct Opened {
    at: usize,
    closed: usize,
    turn: ImportedTurn,
    skipped: Skipped,
}

impl Transcript {
    /// A transcript of `path`, nothing read yet.
    pub fn new(path: &Path) -> Self {
        Transcript {
            path: path.to_path_buf(),
            heads: Heads::default(),
            read_to: 0,
            seal: Vec::new(),
            built: None,
        }
    }

    /// The file this reads.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// **Read what the file gained since the last read** — `true` when it
    /// gained anything. A line not yet ended is still being written: it
    /// waits for the next read. A file that does not go on from what was
    /// read is read again from its start.
    pub fn read_on(&mut self) -> Result<bool, String> {
        let shown = self.path.display().to_string();
        let failed = |e: std::io::Error| format!("{shown} could not be read ({e})");
        let mut file = open(&self.path)?;
        if !self.goes_on(&mut file).map_err(failed)? {
            *self = Transcript::new(&self.path);
        }
        file.seek(SeekFrom::Start(self.read_to)).map_err(failed)?;
        let mut reader = BufReader::new(file);
        let (mut line, mut gained) = (Vec::new(), false);
        loop {
            let read = read_line(&mut reader, &mut line, LINE_CAP).map_err(failed)?;
            if read.spans == 0 || !read.ended {
                break;
            }
            match read.whole {
                true => self.heads.take(&line, self.read_to),
                false => self.heads.unparsable += 1,
            }
            self.read_to += read.spans as u64;
            std::mem::swap(&mut self.seal, &mut line);
            gained = true;
        }
        if gained {
            // The tail of the last line, and only its room: a line of
            // megabytes read last kept its whole allocation for the run, per
            // file kept (review stop 4, B 7).
            let cut = self.seal.len().saturating_sub(SEAL);
            self.seal.drain(..cut);
            self.seal.shrink_to(SEAL);
        }
        Ok(gained)
    }

    /// **A one-shot reading's last line, unended** — whole JSON, taken as a
    /// line; else counted as not JSON. A transcript being read on waits for
    /// the line's end ([`read_on`](Self::read_on)); a reading that will not
    /// read on has no next read to wait for, and dropped it uncounted
    /// (review stop 4, A 6). Claude Code ends every line it writes; another
    /// tool's file, or one cut by hand, might not.
    fn take_unended(&mut self) -> Result<(), String> {
        let shown = self.path.display().to_string();
        let failed = |e: std::io::Error| format!("{shown} could not be read ({e})");
        let mut file = open(&self.path)?;
        file.seek(SeekFrom::Start(self.read_to)).map_err(failed)?;
        let mut rest = Vec::new();
        let read = read_line(&mut BufReader::new(file), &mut rest, LINE_CAP).map_err(failed)?;
        match read.whole {
            true if rest.trim_ascii().is_empty() => return Ok(()),
            true => self.heads.take(&rest, self.read_to),
            false => self.heads.unparsable += 1,
        }
        self.read_to += read.spans as u64;
        Ok(())
    }

    /// Whether `file` goes on from what was read: as long, and ending what
    /// was read with the line read last.
    fn goes_on(&self, file: &mut std::fs::File) -> std::io::Result<bool> {
        if self.read_to == 0 {
            return Ok(true);
        }
        if file.metadata()?.len() < self.read_to {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(self.read_to - self.seal.len() as u64))?;
        let mut end = vec![0; self.seal.len()];
        file.read_exact(&mut end)?;
        Ok(end == self.seal)
    }

    /// **The conversation with every compaction kept**
    /// ([`Scope::Lineage`]), as the file stood at the last
    /// [`read_on`](Self::read_on) — built on from where the last one's last
    /// turn opened, when the chain up to there is the one it was built from
    /// — and how many of its first turns that one had, unchanged: what a
    /// caller made of those, it keeps.
    pub fn lineage(&mut self) -> Result<(&Imported, usize), String> {
        // An entry written again under its own uuid changed what was built:
        // built whole (review stop 4, C — whether Claude Code ever writes one
        // twice, unknown; a build on from before would keep the first).
        if std::mem::take(&mut self.heads.restated) {
            self.built = None;
        }
        let shown = self.path.display();
        let (chain, elsewhere) = self
            .heads
            .walk()
            .ok_or_else(|| format!("{shown} holds no conversation"))?;
        let Window {
            chain,
            earlier,
            leaf,
        } = self.window(chain, elsewhere, Scope::Lineage, None)?;
        // Nothing new on the chain — the file grew off it, or not at all. What
        // every line counts, and not only the chain's, is the file's now.
        if self
            .built
            .as_ref()
            .is_some_and(|built| built.chain == chain)
        {
            let built = self.built.as_mut().expect("just found to be built");
            built.imported.skipped.unparsable = self.heads.unparsable;
            built.imported.cwd.clone_from(&self.heads.cwd);
            return Ok((&built.imported, built.imported.turns.len()));
        }
        let mut file = open(&self.path)?;
        // From where the last build's last turn opened — if the chain up to
        // it is still the one built: a rewind in Claude Code makes another.
        let (from, kept, builder, last) = match self.built.take() {
            Some(Built {
                mut imported,
                chain: before,
                last: Some(opened),
            }) if chain.get(..=opened.at) == before.get(..=opened.at) => {
                imported.turns.truncate(opened.closed);
                let builder = Builder {
                    turns: imported.turns,
                    turn: Some(opened.turn.clone()),
                    skipped: opened.skipped,
                    ..Builder::default()
                };
                (opened.at + 1, opened.closed, builder, Some(opened))
            }
            _ => (0, 0, Builder::default(), None),
        };
        let nodes = &self.heads.nodes;
        let (built, opened) = build_turns(
            &mut file,
            nodes,
            &chain,
            from,
            Scope::Lineage,
            builder,
            &shown,
        )?;
        let built = Built {
            imported: self.imported(built, earlier, leaf),
            chain,
            last: opened.or(last),
        };
        Ok((&self.built.insert(built).imported, kept))
    }

    /// **The conversation as `scope` reads it, ending where `until` says**
    /// ([`read_until`]), from what has been read — built whole.
    fn scoped(&self, scope: Scope, until: Option<&Until>) -> Result<Imported, String> {
        let shown = self.path.display();
        let (chain, elsewhere) = self
            .heads
            .walk()
            .ok_or_else(|| format!("{shown} holds no conversation"))?;
        let window = self.window(chain, elsewhere, scope, until)?;
        let mut file = open(&self.path)?;
        let nodes = &self.heads.nodes;
        let builder = Builder::default();
        let (built, _) = build_turns(&mut file, nodes, &window.chain, 0, scope, builder, &shown)?;
        Ok(self.imported(built, window.earlier, window.leaf))
    }

    /// **The stretch of `chain` a reading takes**, and why it begins after
    /// the conversation's start if it does: ended where `until` says; from
    /// the latest compaction for [`Scope::Resume`], the messages it kept
    /// after its summary; and the parallel calls a single-parent walk
    /// dropped, put back.
    fn window(
        &self,
        mut chain: Vec<String>,
        elsewhere: bool,
        scope: Scope,
        until: Option<&Until>,
    ) -> Result<Window, String> {
        let nodes = &self.heads.nodes;
        let compaction = |uuid: &String| matches!(nodes[uuid].link, Link::Compaction(_));
        if let Some(until) = until {
            let (Until::Before(entry) | Until::After(entry)) = until;
            let at = chain
                .iter()
                .position(|uuid| uuid == entry)
                .ok_or_else(|| format!("{} no longer holds that turn", self.path.display()))?;
            // A turn a summary opens begins at ITS boundary — the one just
            // before it, nothing but Claude Code's own entries between — so
            // the compaction goes with it. A summary with no boundary of its
            // own (copied on) begins its turn itself: the nearest boundary
            // further back cut away every turn after it, the copy holding
            // none (review stop 4, B 3).
            let end = match (until, nodes[entry].summary) {
                (Until::After(_), _) => at + 1,
                (Until::Before(_), true) => own_boundary(&chain[..at], nodes).unwrap_or(at),
                (Until::Before(_), false) => at,
            };
            chain.truncate(end);
        }
        // Taken on the path walked, before the parallel calls are put back:
        // a copy cut after it puts back the same.
        let leaf = chain.last().cloned();
        // What `claude --resume` would hold, there: from the latest
        // compaction — its summary, then the messages it KEPT (Steven, 10-17:
        // "The cost is worth it"), then what followed, as the file a resume
        // writes holds them.
        let start = (scope == Scope::Resume)
            .then(|| chain.iter().rposition(compaction))
            .flatten();
        let earlier = match (start, elsewhere) {
            (Some(_), _) => Some(Earlier::Compacted),
            (None, true) => Some(Earlier::Elsewhere),
            (None, false) => None,
        };
        let kept = start
            .map(|s| kept_before(&chain[..s], self.heads.kept_from.get(&chain[s])))
            .unwrap_or_default();
        let mut chain = chain.split_off(start.unwrap_or(0));
        let after_summary = chain
            .iter()
            .position(|uuid| nodes[uuid].summary)
            .map_or(1, |s| s + 1)
            .min(chain.len());
        chain.splice(after_summary..after_summary, kept);
        let mut seen: HashSet<String> = chain.iter().cloned().collect();
        Ok(Window {
            chain: recover_parallel(nodes, chain, &mut seen),
            earlier,
            leaf,
        })
    }

    /// The reading `built` makes, named by the file.
    fn imported(&self, built: Builder, earlier: Option<Earlier>, leaf: Option<String>) -> Imported {
        let id = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        Imported {
            id,
            cwd: self.heads.cwd.clone(),
            turns: built.turns,
            // A count of the file's lines, not of a chain's entries.
            skipped: Skipped {
                unparsable: self.heads.unparsable,
                ..built.skipped
            },
            earlier,
            leaf,
        }
    }
}

/// What [`Transcript::window`] takes of a chain.
struct Window {
    chain: Vec<String>,
    earlier: Option<Earlier>,
    leaf: Option<String>,
}

impl Heads {
    /// **One whole line of the file**, the one starting at `at`.
    fn take(&mut self, line: &[u8], at: u64) {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            if !line.trim_ascii().is_empty() {
                self.unparsable += 1;
            }
            return;
        };
        let kind = match value["type"].as_str() {
            Some("user") => Kind::User,
            Some("assistant") => Kind::Assistant,
            Some("attachment") => Kind::Attachment,
            Some("system") => Kind::System,
            _ => return,
        };
        let Some(uuid) = value["uuid"].as_str() else {
            return;
        };
        if let Some(dir) = value["cwd"].as_str() {
            self.cwd = Some(dir.to_string());
        }
        let text = |key: &str| value[key].as_str().map(str::to_string);
        let link = match text("parentUuid") {
            Some(parent) => Link::Parent(parent),
            None => link_back(&value, &self.nodes, self.newest.as_deref()),
        };
        let sidechain = value["isSidechain"] == true;
        self.restated |= self.nodes.contains_key(uuid);
        self.nodes.insert(
            uuid.to_string(),
            Node {
                kind,
                link,
                message_id: value["message"]["id"].as_str().map(str::to_string),
                tool_result: kind == Kind::User
                    && value["message"]["content"]
                        .as_array()
                        .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_result")),
                summary: value["isCompactSummary"] == true,
                timestamp: text("timestamp").unwrap_or_default(),
                offset: at,
            },
        );
        if let Some(head) = value["compactMetadata"]["preservedSegment"]["headUuid"].as_str() {
            self.kept_from.insert(uuid.to_string(), head.to_string());
        }
        if !sidechain {
            self.newest = Some(uuid.to_string());
        }
    }

    /// **The whole path back from the leaf**, across every compaction the
    /// file holds — where a reading ends can be any turn of it, and where it
    /// begins depends on where it ends — and whether it ends at a compaction
    /// whose history is in another file ([`Earlier::Elsewhere`]). `None`
    /// when the file holds no conversation.
    fn walk(&self) -> Option<(Vec<String>, bool)> {
        let (mut chain, mut walked, mut elsewhere) = (Vec::new(), HashSet::new(), false);
        let mut at = Some(self.newest.clone()?);
        while let Some(uuid) = at {
            // A cycle stops the walk.
            if !walked.insert(uuid.clone()) {
                break;
            }
            let Some(node) = self.nodes.get(&uuid) else {
                break;
            };
            at = match &node.link {
                Link::Parent(parent) => Some(parent.clone()),
                Link::Compaction(Some(older)) => Some(older.clone()),
                Link::Compaction(None) => {
                    elsewhere = true;
                    None
                }
                Link::Start => None,
            };
            chain.push(uuid);
        }
        chain.reverse();
        Some((chain, elsewhere))
    }
}

/// **The compaction boundary a summary at the end of `before` opens on** —
/// the nearest before it with only Claude Code's own entries (system,
/// attachment) between; `None` when a message comes first.
fn own_boundary(before: &[String], nodes: &HashMap<String, Node>) -> Option<usize> {
    for (at, uuid) in before.iter().enumerate().rev() {
        let node = &nodes[uuid];
        if matches!(node.link, Link::Compaction(_)) {
            return Some(at);
        }
        if matches!(node.kind, Kind::User | Kind::Assistant) {
            return None;
        }
    }
    None
}

/// **A transcript opened to read** — a regular file, never waited on
/// (`knowledge::read::open_regular`: a FIFO swapped in after the listing
/// held a reading thread, or the UI, for good — review stop 4, B 6).
fn open(path: &Path) -> Result<std::fs::File, String> {
    use crate::knowledge::read::{Links, open_regular};
    open_regular(path, Links::Refuse)
        .map_err(|e| format!("{} could not be read ({e})", path.display()))
}

/// **Pass 2: the entries on `chain` from `from` on**, each read from `file`
/// at its offset and built on `built` — and where the last turn it opened
/// opened, for a later build to go on from ([`Opened`]).
fn build_turns(
    file: &mut std::fs::File,
    nodes: &HashMap<String, Node>,
    chain: &[String],
    from: usize,
    scope: Scope,
    mut built: Builder,
    shown: &impl std::fmt::Display,
) -> Result<(Builder, Option<Opened>), String> {
    let mut assistant: Option<Pending> = None;
    let mut last = None;
    for (i, uuid) in chain.iter().enumerate().skip(from) {
        let node = &nodes[uuid];
        let value = read_at(file, node.offset).map_err(|e| format!("{shown}: {e}"))?;
        if node.kind == Kind::Assistant {
            let (text, calls) = assistant_parts(&value, &mut built.skipped);
            match &mut assistant {
                Some(pending) if node.message_id.is_some() && pending.id == node.message_id => {
                    join(&mut pending.text, &text);
                    pending.calls.extend(calls);
                }
                _ => {
                    built.flush(assistant.take());
                    assistant = Some(Pending {
                        id: node.message_id.clone(),
                        text,
                        calls,
                        model: value["message"]["model"].as_str().map(str::to_string),
                    });
                }
            }
            continue;
        }
        built.flush(assistant.take());
        match node.kind {
            Kind::User => {
                let opened = built.opened;
                built.user(&value, scope);
                if built.opened != opened {
                    last = Some(Opened {
                        at: i,
                        closed: built.turns.len(),
                        turn: built.turn.clone().expect("a turn just opened"),
                        skipped: built.skipped,
                    });
                }
            }
            Kind::Attachment => built.skipped.attachments += 1,
            Kind::System => built.skipped.system += 1,
            Kind::Assistant => {}
        }
    }
    built.flush(assistant.take());
    built.close_turn();
    Ok((built, last))
}

/// **Where a session's turns begin as a run of a conversation's** — how an
/// import made before imports recorded their source is found (Steven, 10-16:
/// "Match once by content"). `session` is the Shannon session's turns,
/// oldest first, as `(prompt, reply)`; `lineage` the conversation read with
/// [`Scope::Lineage`], where an import's first turn — a summary of the
/// compaction latest at import — still is. A run starts where the session's
/// first turn matches one of the conversation's, prompt and reply, and goes
/// on while the next ones do; the longest wins, the later on a tie (the
/// newer compaction). `(first, count)`, or `None`.
///
/// `same(session_text, conversation_text)` decides a match: the importer
/// scrubbed secrets on the way in, so the caller compares as it imports.
pub fn find_run(
    lineage: &[ImportedTurn],
    session: &[(String, String)],
    same: impl Fn(&str, &str) -> bool,
) -> Option<(usize, usize)> {
    let matches = |turn: &ImportedTurn, (prompt, reply): &(String, String)| {
        same(prompt, &turn.prompt) && same(reply, &turn.reply)
    };
    let first = session.first()?;
    (0..lineage.len())
        .filter(|&at| matches(&lineage[at], first))
        .map(|at| {
            let run = lineage[at..]
                .iter()
                .zip(session)
                .take_while(|(turn, said)| matches(turn, said))
                .count();
            (at, run)
        })
        .max_by_key(|&(at, run)| (run, at))
}

/// **The parallel calls a single-parent walk dropped, put back.** Parallel
/// calls make the chain a DAG (the module's account above: each result
/// parented on its own one-block entry), so the walk keeps one branch. For
/// each `message.id` group touching the chain: its
/// off-chain siblings, then the off-chain results of every member, spliced
/// after the group's last on-chain member, by timestamp (the file's order on
/// a tie) — so a reply's blocks stay together and each result follows its
/// call.
fn recover_parallel(
    nodes: &HashMap<String, Node>,
    chain: Vec<String>,
    seen: &mut HashSet<String>,
) -> Vec<String> {
    let mut siblings: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut results_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for (uuid, node) in nodes {
        match (node.kind, &node.message_id, &node.link) {
            (Kind::Assistant, Some(id), _) => {
                siblings.entry(id.as_str()).or_default().push(uuid.as_str())
            }
            (Kind::User, _, Link::Parent(parent)) if node.tool_result => results_of
                .entry(parent.as_str())
                .or_default()
                .push(uuid.as_str()),
            _ => {}
        }
    }
    let mut anchor: HashMap<&str, &str> = HashMap::new();
    for uuid in &chain {
        let node = &nodes[uuid];
        if let (Kind::Assistant, Some(id)) = (node.kind, &node.message_id) {
            anchor.insert(id.as_str(), uuid.as_str());
        }
    }
    let by_time = |a: &&str, b: &&str| {
        let (a, b) = (&nodes[*a], &nodes[*b]);
        (&a.timestamp, a.offset).cmp(&(&b.timestamp, b.offset))
    };
    let mut inserts: HashMap<String, Vec<String>> = HashMap::new();
    let mut done: HashSet<&str> = HashSet::new();
    for uuid in &chain {
        let node = &nodes[uuid];
        let (Kind::Assistant, Some(id)) = (node.kind, &node.message_id) else {
            continue;
        };
        if !done.insert(id.as_str()) {
            continue;
        }
        let group = siblings.get(id.as_str()).cloned().unwrap_or_default();
        let mut lost: Vec<&str> = group
            .iter()
            .copied()
            .filter(|s| !seen.contains(*s))
            .collect();
        let mut results: Vec<&str> = group
            .iter()
            .flat_map(|member| results_of.get(member).into_iter().flatten().copied())
            .filter(|r| !seen.contains(*r))
            .collect();
        if lost.is_empty() && results.is_empty() {
            continue;
        }
        lost.sort_by(&by_time);
        results.sort_by(&by_time);
        let recovered: Vec<String> = lost
            .into_iter()
            .chain(results)
            .map(str::to_string)
            .collect();
        seen.extend(recovered.iter().cloned());
        inserts.insert(anchor[id.as_str()].to_string(), recovered);
    }
    let mut out = Vec::with_capacity(chain.len());
    for uuid in chain {
        let extra = inserts.remove(&uuid);
        out.push(uuid);
        out.extend(extra.into_iter().flatten());
    }
    out
}

/// The entry whose line starts at `offset`.
fn read_at(file: &mut std::fs::File, offset: u64) -> Result<Value, String> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("could not be re-read ({e})"))?;
    let mut line = Vec::new();
    let read = read_line(&mut BufReader::new(&mut *file), &mut line, LINE_CAP)
        .map_err(|e| format!("could not be re-read ({e})"))?;
    if !read.whole {
        return Err("changed while it was read (a line past the longest read)".into());
    }
    serde_json::from_slice(&line).map_err(|e| format!("changed while it was read ({e})"))
}

/// An assistant message being gathered from its entries.
struct Pending {
    id: Option<String>,
    text: String,
    calls: Vec<Block>,
    model: Option<String>,
}

/// An assistant entry's text and tool calls; anything else counted.
fn assistant_parts(value: &Value, skipped: &mut Skipped) -> (String, Vec<Block>) {
    let (mut text, mut calls) = (String::new(), Vec::new());
    match &value["message"]["content"] {
        Value::Array(blocks) => {
            for block in blocks {
                match block["type"].as_str() {
                    Some("text") => join(&mut text, block["text"].as_str().unwrap_or("")),
                    Some("tool_use") => match block["id"].as_str().filter(|id| !id.is_empty()) {
                        Some(id) => calls.push(Block::ToolUse {
                            id: id.to_string(),
                            name: block["name"].as_str().unwrap_or("").to_string(),
                            input: block["input"].clone(),
                        }),
                        None => skipped.unknown += 1,
                    },
                    Some("thinking" | "redacted_thinking") => skipped.thinking += 1,
                    _ => skipped.unknown += 1,
                }
            }
        }
        Value::String(s) => join(&mut text, s),
        _ => skipped.unknown += 1,
    }
    (text, calls)
}

/// `more` after `text`, a paragraph apart.
fn join(text: &mut String, more: &str) {
    if more.is_empty() {
        return;
    }
    if !text.is_empty() {
        text.push_str("\n\n");
    }
    text.push_str(more);
}

/// **Turns, built in the order a live one is** — prompt, rounds, reply.
#[derive(Default)]
struct Builder {
    turns: Vec<ImportedTurn>,
    turn: Option<ImportedTurn>,
    /// The assistant's text since the last round closed.
    text: String,
    /// Calls waiting for their results, and the results so far.
    calls: Vec<Block>,
    results: Vec<Block>,
    skipped: Skipped,
    /// How many turns a prompt has opened — how `build_turns` sees one open.
    opened: usize,
}

impl Builder {
    /// A whole assistant message.
    fn flush(&mut self, message: Option<Pending>) {
        let Some(message) = message else {
            return;
        };
        // **Shannon's own handoff, before anything else**: the history a
        // switch to Claude Code seeded, as one line of text. It is a copy of
        // turns Shannon holds as turns, so imported it was a wall of
        // `[Assistant ran Edit: {json}]` under a prompt saying "earlier"
        // (Steven, 10-05, image 91). No turn open here is no turn yet: one
        // closes only as the next opens, or after the last message.
        if self.turn.is_none()
            && message
                .text
                .starts_with(crate::backend::claude_code::HANDOFF)
        {
            self.skipped.handoffs += 1;
            return;
        }
        let turn = self.turn.get_or_insert_with(|| ImportedTurn {
            prompt: EARLIER.to_string(),
            ..ImportedTurn::default()
        });
        if turn.model.is_none() {
            turn.model = message.model;
        }
        // A reply after results closes the round they answered.
        self.close_round();
        join(&mut self.text, &message.text);
        self.calls.extend(message.calls);
    }

    /// A user entry: a prompt, tool results, or Claude Code's own.
    fn user(&mut self, value: &Value, scope: Scope) {
        if value["isMeta"] == true {
            self.skipped.meta += 1;
            return;
        }
        // Reading the history it summarises, the summary is a second copy —
        // except where the summary IS the point: `Lineage` keeps it.
        let summary = value["isCompactSummary"] == true;
        if summary && scope == Scope::Everything {
            self.skipped.summaries += 1;
            return;
        }
        let uuid = value["uuid"].as_str().map(str::to_string);
        let (mut text, mut results) = (String::new(), Vec::new());
        match &value["message"]["content"] {
            Value::String(s) => join(&mut text, s),
            Value::Array(blocks) => {
                for block in blocks {
                    match block["type"].as_str() {
                        Some("tool_result") => results.push(self.tool_result(block)),
                        Some("text") => join(&mut text, block["text"].as_str().unwrap_or("")),
                        Some("image") => {
                            self.skipped.images += 1;
                            join(&mut text, IMAGE);
                        }
                        _ => self.skipped.unknown += 1,
                    }
                }
            }
            _ => self.skipped.unknown += 1,
        }
        if results.is_empty() {
            if !text.is_empty() {
                self.prompt(text, uuid, summary);
            }
            return;
        }
        // Text beside results is about them ("[Request interrupted…]"), not
        // a new prompt: it stays with the last.
        if let (false, Some(Block::ToolResult { content, .. })) =
            (text.is_empty(), results.last_mut())
        {
            join(content, &text);
        }
        for result in results {
            if self.calls.is_empty() {
                self.skipped.unknown += 1; // a result for no call
            } else {
                self.results.push(result);
            }
        }
    }

    fn tool_result(&mut self, block: &Value) -> Block {
        let content = match &block["content"] {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|part| match part["type"].as_str() {
                    Some("text") => part["text"].as_str().map(str::to_string),
                    Some("image") => {
                        self.skipped.images += 1;
                        Some(IMAGE.to_string())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Value::Null => String::new(),
            other => other.to_string(),
        };
        Block::ToolResult {
            tool_use_id: block["tool_use_id"].as_str().unwrap_or("").to_string(),
            content: self.counted(content),
            is_error: block["is_error"].as_bool().unwrap_or(false),
        }
    }

    /// `content`, whole — counted when a reader will cut it ([`cut_result`],
    /// once scrubbed).
    fn counted(&mut self, content: String) -> String {
        self.skipped.cut += usize::from(content.len() > RESULT_CAP);
        content
    }

    /// A prompt: a new turn — or, before the current one has any reply,
    /// more of its prompt (a summary and the question after it), the turn
    /// still the one its first entry opened, the joining entry recorded.
    fn prompt(&mut self, text: String, uuid: Option<String>, summary: bool) {
        match self.turn.as_mut() {
            Some(turn)
                if turn.rounds.is_empty() && self.text.is_empty() && self.calls.is_empty() =>
            {
                join(&mut turn.prompt, &text);
                turn.joined.extend(uuid);
            }
            _ => {
                self.close_turn();
                self.opened += 1;
                self.turn = Some(ImportedTurn {
                    prompt: text,
                    uuid,
                    summary,
                    ..ImportedTurn::default()
                });
            }
        }
    }

    /// Close the open round, every call answered: one with no result is
    /// answered as having none, since the API refuses a `tool_use` without.
    fn close_round(&mut self) {
        if self.calls.is_empty() {
            return;
        }
        // By a set, once each: a scan of the results for every call was
        // quadratic in a round — 312 ms at 8k calls (review stop 4, E 6). An
        // id answered here is answered for a call repeating it, as before.
        let mut answered: HashSet<String> = self
            .results
            .iter()
            .filter_map(|r| match r {
                Block::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                _ => None,
            })
            .collect();
        for call in &self.calls {
            let Block::ToolUse { id, .. } = call else {
                continue;
            };
            if answered.insert(id.clone()) {
                self.results.push(Block::ToolResult {
                    tool_use_id: id.clone(),
                    content: UNANSWERED.to_string(),
                    is_error: true,
                });
                self.skipped.unanswered += 1;
            }
        }
        let round = Round {
            text: std::mem::take(&mut self.text),
            calls: std::mem::take(&mut self.calls),
            results: std::mem::take(&mut self.results),
        };
        if let Some(turn) = self.turn.as_mut() {
            turn.rounds.push(round);
        }
    }

    fn close_turn(&mut self) {
        self.close_round();
        if let Some(mut turn) = self.turn.take() {
            turn.reply = std::mem::take(&mut self.text);
            self.turns.push(turn);
        }
    }
}

#[cfg(test)]
mod tests;
