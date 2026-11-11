//! **The Claude Code sessions Shannon started** — which history each one
//! saw, kept across runs.
//!
//! A `claude -p` turn runs in a Claude Code session; the next turn on the
//! same history resumes it (forked, so the one filed stays as filed). Kept in
//! memory only, the record went with the process: a restart's first turn
//! seeded a FRESH session with the whole history as one handoff line —
//! paying for every token again, cache cold. And every one of those sessions
//! is a transcript file in Claude Code's directory, which `:graph` then
//! listed as a Claude Code conversation though it is a Shannon session's
//! (Steven, 10-04: "those claude code sessions there are effectively shannon
//! sessions, because they were created with shannon").
//!
//! So the record is kept: one line per session, `{history digest, session
//! id}`, appended, owner-only, in Shannon's state directory — which the gate
//! protects as Shannon's own state. The digest is blake3 over the history's
//! JSON, the same in every build (the hasher it replaces was not).
//!
//! **Two facts, not one**: which sessions Shannon STARTED — every one Claude
//! Code names to it, a turn's, a job's, a distillation's, a stopped or failed
//! turn's — and which of them a turn may RESUME — a finished transcript
//! turn's, filed under the history it leaves. The second alone answered the
//! first, and every session a summary or a broken turn ran in was listed by
//! `:graph` as a conversation of Claude Code's own (Steven's screenshot,
//! 10-16: a `Summarize this conversation…` row). A started session is noted
//! as one line `{session}`; an older build skips it (its lines need both).
//!
//! **And one more**: which of Shannon's OWN sessions have been matched
//! against Claude Code's conversations — once each, ever
//! (`App::match_old_imports`): an import made before imports recorded
//! their source is found by its turns, and the reading costs a pass over
//! every transcript here. One line `{checked: episode}`, skipped by any
//! build before it.
//!
//! **And whether Shannon may read them at all** — Claude Code's
//! conversations are another program's records, and Shannon reads them
//! only where the user said it may (Steven, 10-17: ASK before reading
//! them, "So they don't think we're doing anything sneaky here"; 10-18,
//! the shape: once per directory, `y`, `n` or `always`). `always` is one
//! line `{read: directory}`, for good; `y` and `n` hold for this run.
//! `:unread` takes it back with one more, `{unread: directory}` — the file
//! is only ever appended to, as every Shannon running appends.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use shannon_core::Message;

/// **A directory as leave to read is keyed** — absolute, its `.` and `..`
/// folded away by name: what is recorded and what is asked of the record
/// go through this one spelling. `std::path::absolute` keeps `..`, and
/// `Path::starts_with` compares components, so `-here/../-there` passed
/// leave for `-here` (review stop 5, A 2, B 6, C 1). By name, not
/// `canonicalize`: a directory is the one the user was asked about.
pub fn normal(dir: &Path) -> PathBuf {
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut out = PathBuf::new();
    for part in dir.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The file the record is kept in: `$XDG_STATE_HOME/shannon/claude_sessions`.
pub fn default_path() -> Option<PathBuf> {
    shannon_core::Config::state_dir().map(|dir| dir.join("claude_sessions"))
}

/// **A history as one key**, the same in every build. `None` when the
/// history cannot be serialised — and then nothing is filed or resumed: a
/// fallback key would be one shared by every such history, and a shared key
/// is how a session would resume context it never saw.
pub fn digest(history: &[Message]) -> Option<String> {
    let text = serde_json::to_string(history).ok()?;
    Some(blake3::hash(text.as_bytes()).to_hex().to_string())
}

/// The sessions Shannon started, the resumable ones by the history each saw —
/// and where they are kept (`None`: in memory only, a test app or a machine
/// with no state directory).
#[derive(Debug, Clone, Default)]
pub struct ClaudeSessions {
    file: Option<PathBuf>,
    by_history: HashMap<String, String>,
    /// Every session Shannon started, filed under a history or not.
    started: HashSet<String>,
    /// Shannon's sessions, and Claude Code's conversations, already matched
    /// — each by the matcher that did it ([`MATCHER`]).
    checked: HashMap<String, u32>,
    /// The directories whose conversations the user let Shannon read — for
    /// good (`true`: `always`, kept) or for this run (`y`) — keyed by
    /// [`normal`]. Each holds itself; only Claude Code's projects directory
    /// holds what is under it ([`may_read`](Self::may_read)).
    allowed: HashMap<PathBuf, bool>,
    /// Those the user said `n` to, this run: not asked again unprompted.
    declined: HashSet<PathBuf>,
}

/// **What the user said about reading a directory of Claude Code's
/// conversations** ([`ClaudeSessions::may_read`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consent {
    /// `y` this run, or `always`: read.
    Given,
    /// `n` this run: not read, and not asked again unless the user asks.
    Declined,
    /// Nothing yet: ask before reading.
    Unasked,
}

/// **Which matcher checked a session or a conversation** — raised when the
/// matcher learns something, so what an older one missed is looked at again,
/// once, by the new one. 1: the first (`40f64b9`), which left two of Steven's
/// four old imports unfound and said nothing of why (10-16). 2: said why,
/// and a distillation is Shannon's. 3: a reading crosses a compaction that
/// kept its last messages — 2 read such a conversation from its latest
/// compaction only, where no older import's first turn is (10-17).
pub const MATCHER: u32 = 3;

/// One line of the file, read.
enum Line {
    /// `{"history": …, "session": …}`, a session to resume, or
    /// `{"session": …}`, one only noted as Shannon's.
    Session(Option<String>, String),
    /// `{"checked": …, "by": N}`, matched by matcher N (1 when unsaid).
    Checked(String, u32),
    /// `{"read": directory}`, the user's `always`.
    Read(PathBuf),
    /// `{"unread": directory}`, an `always` taken back (`:unread`).
    Unread(PathBuf),
}

fn entry(line: &str) -> Option<Line> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if let Some(dir) = value["read"].as_str() {
        return Some(Line::Read(PathBuf::from(dir)));
    }
    if let Some(dir) = value["unread"].as_str() {
        return Some(Line::Unread(PathBuf::from(dir)));
    }
    if let Some(id) = value["checked"].as_str() {
        let by = value["by"].as_u64().and_then(|n| u32::try_from(n).ok());
        return Some(Line::Checked(id.to_string(), by.unwrap_or(1)));
    }
    Some(Line::Session(
        value["history"].as_str().map(str::to_string),
        value["session"].as_str()?.to_string(),
    ))
}

impl ClaudeSessions {
    /// **Read from `file`**, or why not — a file that cannot be read is said,
    /// and resumes nothing (each history then starts a fresh session, which
    /// is correct, only dearer). A line that does not parse is skipped.
    pub fn load(file: Option<PathBuf>) -> (ClaudeSessions, Option<String>) {
        let Some(path) = file.clone() else {
            return (ClaudeSessions::default(), None);
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                return (
                    ClaudeSessions {
                        file,
                        ..ClaudeSessions::default()
                    },
                    Some(format!(
                        "the Claude Code sessions record ({}) could not be read ({e}) — each \
                         conversation starts a fresh one",
                        path.display()
                    )),
                );
            }
        };
        let mut sessions = ClaudeSessions {
            file,
            ..ClaudeSessions::default()
        };
        for line in text.lines().filter_map(entry) {
            match line {
                Line::Session(history, session) => {
                    sessions.started.insert(session.clone());
                    if let Some(history) = history {
                        sessions.by_history.insert(history, session);
                    }
                }
                Line::Checked(id, by) => {
                    let best = sessions.checked.entry(id).or_insert(by);
                    *best = (*best).max(by);
                }
                // In the order written: a later `:unread` takes back an
                // earlier `always`, and a later `always` gives it again.
                Line::Read(dir) => {
                    sessions.allowed.insert(normal(&dir), true);
                }
                Line::Unread(dir) => {
                    sessions.allowed.remove(&normal(&dir));
                }
            }
        }
        (sessions, None)
    }

    /// **May Shannon read the conversations in `dir`?** Given when the user
    /// allowed `dir` itself, or `projects` — Claude Code's projects
    /// directory, which holds every project's — with `dir` under it; never
    /// by leave for some other directory above it: `a` to an import's
    /// question about the home directory held every project, for good
    /// (review stop 5, A 1). Declined only as itself.
    pub fn may_read(&self, dir: &Path, projects: Option<&Path>) -> Consent {
        let (dir, projects) = (normal(dir), projects.map(normal));
        let holds = |allowed: &PathBuf| {
            *allowed == dir || (projects.as_ref() == Some(allowed) && dir.starts_with(allowed))
        };
        if self.allowed.keys().any(holds) {
            Consent::Given
        } else if self.declined.contains(&dir) {
            Consent::Declined
        } else {
            Consent::Unasked
        }
    }

    /// **The user let Shannon read `dir`** — for this run, or `always`:
    /// then kept, one line (a path that is not UTF-8 is kept as near as it
    /// can be, and a later run asks again). Allowed in memory whatever
    /// happens to the write; its failure is returned, for the caller to say.
    pub fn allow(&mut self, dir: &Path, always: bool) -> Result<(), String> {
        // An `n` before it stands, and is outweighed: leave is asked first.
        // Kept is kept until taken back: a `y` after an `always` stays one.
        let dir = normal(dir);
        *self.allowed.entry(dir.clone()).or_default() |= always;
        let (true, Some(path)) = (always, &self.file) else {
            return Ok(());
        };
        let line = serde_json::json!({"read": dir.to_string_lossy()}).to_string();
        append(path, &line)
            .map_err(|e| format!("read for this run only — `always` was not recorded ({e})"))
    }

    /// **The user said `n` to reading `dir`**, for this run.
    pub fn decline(&mut self, dir: &Path) {
        self.declined.insert(normal(dir));
    }

    /// **The user took back Shannon's leave to read `dir`** (`:unread`) —
    /// as `n` for this run, and an `always` taken back for good, one line.
    /// Whether `dir` itself held leave (a directory holding it may still);
    /// the write's failure is returned, as [`allow`](Self::allow)'s.
    pub fn take_back(&mut self, dir: &Path) -> Result<bool, String> {
        self.decline(dir);
        let dir = normal(dir);
        let Some(kept) = self.allowed.remove(&dir) else {
            return Ok(false);
        };
        let (true, Some(path)) = (kept, &self.file) else {
            return Ok(true);
        };
        let line = serde_json::json!({"unread": dir.to_string_lossy()}).to_string();
        // The record's failure alone: what this run still reads is the
        // caller's to say (review stop 5, E1 F4 — "not read this run" was
        // said beside "still read").
        append(path, &line).map(|()| true).map_err(|e| {
            format!(
                "`always` for {} was not taken back for the next run ({e})",
                dir.display()
            )
        })
    }

    /// The directories the user let Shannon read, this run or for good.
    pub fn allowed(&self) -> Vec<PathBuf> {
        self.allowed.keys().cloned().collect()
    }

    /// **Every `always` the record holds now**, added to this run's leave —
    /// what `:unread` must take back: another Shannon running beside this
    /// one may have said `always` since this one loaded the record, and
    /// `:unread` here took back only what this one remembered — the next
    /// launch read there unasked (review stop 5, E1 F1). The other way
    /// round is not looked for: an `:unread` there leaves this run reading
    /// until it ends.
    pub fn reread_always(&mut self) {
        let Some(text) = self
            .file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
        else {
            return;
        };
        let mut kept = HashSet::new();
        for line in text.lines().filter_map(entry) {
            match line {
                Line::Read(dir) => {
                    kept.insert(normal(&dir));
                }
                Line::Unread(dir) => {
                    kept.remove(&normal(&dir));
                }
                Line::Session(..) | Line::Checked(..) => {}
            }
        }
        for dir in kept {
            self.allowed.insert(dir, true);
        }
    }

    /// The session that last saw exactly `history`, to resume.
    pub fn get(&self, history: &[Message]) -> Option<&str> {
        self.by_history.get(&digest(history)?).map(String::as_str)
    }

    /// **File `session` under `history`**, and keep it. Kept in memory
    /// whatever happens to the write; the write's failure is returned, for
    /// the caller to say.
    pub fn file(&mut self, history: &[Message], session: String) -> Result<(), String> {
        let Some(key) = digest(history) else {
            return Ok(());
        };
        let line = serde_json::json!({"history": key, "session": session}).to_string();
        self.started.insert(session.clone());
        self.by_history.insert(key, session);
        self.write(&line)
    }

    /// **Note that Shannon started `session`** — when Claude Code names it,
    /// so whatever the work is and however it ends. Not filed under a
    /// history, so nothing resumes it; noted, so `:graph` does not list it as
    /// a conversation of Claude Code's own. Written once a session; the
    /// write's failure is returned, as [`file`](Self::file)'s.
    pub fn note(&mut self, session: &str) -> Result<(), String> {
        if !self.started.insert(session.to_string()) {
            return Ok(());
        }
        self.write(&serde_json::json!({"session": session}).to_string())
    }

    /// **Did Shannon start this Claude Code session?** — what `:graph` asks,
    /// so a conversation that is a Shannon session's is not listed again as
    /// Claude Code's.
    pub fn started(&self, id: &str) -> bool {
        self.started.contains(id)
    }

    /// **Has `id` — a Shannon session (hex) or a Claude Code conversation —
    /// been matched by this [`MATCHER`]?** Then it is not matched again; by
    /// an older one, it is.
    pub fn checked(&self, id: &str) -> bool {
        self.checked.get(id).is_some_and(|&by| by >= MATCHER)
    }

    /// **Record that `id` was matched**, found or not, by this [`MATCHER`] —
    /// written once; the write's failure is returned, as
    /// [`file`](Self::file)'s.
    pub fn check(&mut self, id: &str) -> Result<(), String> {
        if self.checked(id) {
            return Ok(());
        }
        self.checked.insert(id.to_string(), MATCHER);
        self.write(&serde_json::json!({"checked": id, "by": MATCHER}).to_string())
    }

    /// Is `session` filed for a turn to resume — under any history?
    #[cfg(test)]
    pub(crate) fn filed(&self, session: &str) -> bool {
        self.by_history.values().any(|s| s == session)
    }

    /// Append `line` to the record, if it is kept anywhere.
    fn write(&self, line: &str) -> Result<(), String> {
        match &self.file {
            Some(path) => append(path, line)
                .map_err(|e| format!("the Claude Code session was not recorded — {e}")),
            None => Ok(()),
        }
    }
}

/// Append `line` to `path`, owner-only, creating it (and its directory,
/// `0700`) if need be.
fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        crate::private_file::owner_only_dir(parent)
            .map_err(|e| std::io::Error::other(format!("{e:#}")))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    // ONE write, the line and its end together: `writeln!` on a `File` made
    // two, and two Shannons appending at once could glue a line to another
    // — which `entry` then skips, a lost `{unread}` reading again unasked
    // (review stop 5, B 8).
    opts.open(path)?.write_all(format!("{line}\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn history(words: &[&str]) -> Vec<Message> {
        words.iter().map(|w| Message::user(*w)).collect()
    }

    /// **Filed, kept owner-only, and read back by the next run** — the same
    /// history finds the same session; another history finds none.
    #[test]
    fn a_session_is_found_again_by_the_next_run() {
        use std::os::unix::fs::PermissionsExt;
        let state = TempDir::new().unwrap();
        crate::private_file::assert_umask_grants_more(state.path());
        let file = state.path().join("claude_sessions");
        let (mut sessions, note) = ClaudeSessions::load(Some(file.clone()));
        assert_eq!(note, None, "no file yet is nothing to say");
        sessions
            .file(&history(&["a", "b"]), "sess-1".into())
            .unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let (next_run, _) = ClaudeSessions::load(Some(file));
        assert_eq!(next_run.get(&history(&["a", "b"])), Some("sess-1"));
        assert_eq!(next_run.get(&history(&["a"])), None);
        assert!(next_run.started("sess-1"));
        assert!(!next_run.started("sess-2"));
    }

    /// **Noted is started, never resumable** — a session Shannon ran but did
    /// not file (a distillation's, a failed turn's) reads back as Shannon's,
    /// and no history finds it. Written once however often it is noted; a
    /// noted line and a filed line in one file both read.
    #[test]
    fn a_noted_session_is_started_and_never_resumed() {
        let state = TempDir::new().unwrap();
        let file = state.path().join("claude_sessions");
        let (mut sessions, _) = ClaudeSessions::load(Some(file.clone()));
        sessions.note("sess-summary").unwrap();
        sessions.note("sess-summary").unwrap();
        sessions.note("sess-turn").unwrap();
        sessions.file(&history(&["a"]), "sess-turn".into()).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            text.matches("sess-summary").count(),
            1,
            "noted once: {text}"
        );

        let (next_run, said) = ClaudeSessions::load(Some(file));
        assert_eq!(said, None);
        assert!(next_run.started("sess-summary"), "{next_run:?}");
        assert!(next_run.started("sess-turn"), "{next_run:?}");
        assert!(!next_run.filed("sess-summary"), "{next_run:?}");
        assert_eq!(next_run.get(&history(&["a"])), Some("sess-turn"));
    }

    /// **A check by an older matcher is no check** — what it left unfound is
    /// looked at again, once, by the new one; and a check line from before
    /// matchers were numbered is the first's.
    #[test]
    fn a_check_by_an_older_matcher_is_looked_at_again() {
        let state = TempDir::new().unwrap();
        let file = state.path().join("claude_sessions");
        std::fs::write(
            &file,
            "{\"checked\":\"old\"}\n{\"checked\":\"older\",\"by\":1}\n",
        )
        .unwrap();
        let (mut sessions, _) = ClaudeSessions::load(Some(file.clone()));
        assert!(!sessions.checked("old") && !sessions.checked("older"));
        sessions.check("old").unwrap();
        assert!(sessions.checked("old"));
        let (next_run, _) = ClaudeSessions::load(Some(file));
        assert!(next_run.checked("old"), "by this matcher, kept");
        assert!(!next_run.checked("older"));
    }

    /// **The record and the directory it makes are the owner's alone** —
    /// `0600` and `0700`, whatever the umask: the directory was made with
    /// the umask's mode while the doc said `0700` (review stop 3, B).
    #[cfg(unix)]
    #[test]
    fn the_record_and_its_directory_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let state = TempDir::new().unwrap();
        crate::private_file::assert_umask_grants_more(state.path());
        let dir = state.path().join("made").join("here");
        let file = dir.join("claude_sessions");
        let (mut sessions, _) = ClaudeSessions::load(Some(file.clone()));
        sessions.note("a-session").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&file), 0o600);
    }

    /// **`always` is kept, `y` and `n` are this run's** — and a directory
    /// allowed holds what is under it (the leave every directory's listing
    /// took, before 10-21, holds each project's), never what beside it.
    #[test]
    fn leave_to_read_is_kept_only_when_always() {
        let state = TempDir::new().unwrap();
        let file = state.path().join("claude_sessions");
        let projects = Path::new("/claude/projects");
        let (here, there) = (projects.join("-here"), projects.join("-there"));
        let (mut sessions, _) = ClaudeSessions::load(Some(file.clone()));
        assert_eq!(sessions.may_read(&here, Some(projects)), Consent::Unasked);
        sessions.decline(&here);
        assert_eq!(sessions.may_read(&here, Some(projects)), Consent::Declined);
        sessions.allow(&here, false).unwrap();
        assert_eq!(
            sessions.may_read(&here, Some(projects)),
            Consent::Given,
            "`y` after `n`"
        );
        assert_eq!(
            sessions.may_read(&there, Some(projects)),
            Consent::Unasked,
            "beside it"
        );
        sessions.allow(&there, true).unwrap();

        let (next_run, said) = ClaudeSessions::load(Some(file.clone()));
        assert_eq!(said, None);
        assert_eq!(
            next_run.may_read(&there, Some(projects)),
            Consent::Given,
            "`always`, kept"
        );
        assert_eq!(
            next_run.may_read(&here, Some(projects)),
            Consent::Unasked,
            "`y`, this run's"
        );

        let (mut sessions, _) = ClaudeSessions::load(Some(file.clone()));
        sessions.allow(projects, true).unwrap();
        let (next_run, _) = ClaudeSessions::load(Some(file));
        assert_eq!(
            next_run.may_read(&here, Some(projects)),
            Consent::Given,
            "under every directory"
        );
        assert_eq!(
            next_run.may_read(Path::new("/claude"), Some(projects)),
            Consent::Unasked,
            "not above it"
        );
    }

    /// **`:unread` takes `always` back for good, and is `n` for this run** —
    /// one line, read in order: an `always` after it gives leave again. A
    /// `y` taken back writes nothing; one under a directory still allowed is
    /// still read.
    #[test]
    fn leave_taken_back_is_kept_and_read_in_order() {
        let state = TempDir::new().unwrap();
        let file = state.path().join("claude_sessions");
        let projects = Path::new("/claude/projects");
        let (here, there) = (projects.join("-here"), projects.join("-there"));
        let (mut sessions, _) = ClaudeSessions::load(Some(file.clone()));
        sessions.allow(&here, true).unwrap();
        sessions.allow(&here, false).unwrap(); // a `y` after: still kept
        sessions.allow(&there, false).unwrap();
        assert_eq!(sessions.take_back(&here), Ok(true));
        assert_eq!(sessions.take_back(&there), Ok(true));
        assert_eq!(sessions.take_back(&there), Ok(false), "nothing left");
        assert_eq!(
            sessions.may_read(&here, Some(projects)),
            Consent::Declined,
            "`n`, this run"
        );
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text.lines().count(), 2, "`always`, then taken back: {text}");

        let (mut next_run, _) = ClaudeSessions::load(Some(file.clone()));
        assert_eq!(
            next_run.may_read(&here, Some(projects)),
            Consent::Unasked,
            "asked again"
        );
        next_run.allow(&here, true).unwrap();
        let (mut after, _) = ClaudeSessions::load(Some(file.clone()));
        assert_eq!(
            after.may_read(&here, Some(projects)),
            Consent::Given,
            "given again, after"
        );

        after.allow(projects, true).unwrap();
        assert_eq!(after.take_back(&here), Ok(true));
        assert_eq!(
            after.may_read(&here, Some(projects)),
            Consent::Given,
            "held by every directory's"
        );
    }

    /// **Leave is its own directory's; only Claude Code's projects directory
    /// holds what is under it** (review stop 5, A 1: `a` to an import's
    /// question about an ancestor held every project, for good) — and `..`
    /// is folded by name, the same for what is recorded and what is asked
    /// (A 2, B 6, C 1).
    #[test]
    fn leave_holds_its_own_directory_and_folds_dot_dot() {
        let projects = Path::new("/claude/projects");
        let (here, there) = (projects.join("-here"), projects.join("-there"));
        let mut sessions = ClaudeSessions::default();
        let asked = |s: &ClaudeSessions, dir: &Path| s.may_read(dir, Some(projects));
        sessions.allow(Path::new("/claude"), false).unwrap();
        assert_eq!(
            asked(&sessions, &here),
            Consent::Unasked,
            "an ancestor holds nothing under it"
        );
        sessions.allow(&here, false).unwrap();
        assert_eq!(
            asked(&sessions, &here.join("sub")),
            Consent::Unasked,
            "nor a project"
        );
        assert_eq!(
            asked(&sessions, &here.join("../-there")),
            Consent::Unasked,
            "`..` folded: that is -there"
        );
        // Recorded by the same spelling: a `..` given is the directory it
        // lands in (a trailing `.` would prove nothing — `Path`'s equality
        // drops it).
        sessions.allow(&here.join("../-there"), false).unwrap();
        assert_eq!(asked(&sessions, &there), Consent::Given, "one spelling");
        sessions.allow(projects, false).unwrap();
        assert_eq!(
            asked(&sessions, &here.join("sub")),
            Consent::Given,
            "every project's"
        );
        assert_eq!(asked(&sessions, Path::new("/elsewhere")), Consent::Unasked);
    }

    /// The digest is the history's, not the process's: two computations
    /// agree, and a different history differs.
    ///
    /// **And the same in every build** — the known answer, pinned: two calls
    /// in one process agree under any deterministic hash, so that half
    /// measured nothing (review stop 12, T7 15). It is what the next run
    /// looks a session up by, so a change to how a message serializes —
    /// which moves it — loses every session filed before; with it pinned,
    /// that is a decision someone makes, not a surprise. (Pinned from the
    /// build of 10-27: a regression pin, not a value computed apart.)
    #[test]
    fn the_digest_is_stable_and_discriminating() {
        let a = digest(&history(&["x"])).unwrap();
        assert_eq!(
            a, "2a8e3ea4e35eb93dc73627fce23515fef19db365f32ddf5c1d55b86f79c1f99f",
            "the digest of one message moved: every session filed is lost to the next run"
        );
        assert_eq!(Some(a.clone()), digest(&history(&["x"])));
        assert_ne!(Some(a), digest(&history(&["y"])));
    }
}
