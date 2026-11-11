//! **Shannon's own file tools** — `Read`, `Edit` and `Write`, for the API
//! backend (`docs/DesignDocs/file-tools.md`). They take Claude Code's names
//! and required arguments (S21-Q25), so a record, the pane's change, a diff
//! and read-first read one vocabulary whichever backend ran the call; the
//! optional arguments are Shannon's to add.
//!
//! - [`Call`]: a call's input, read, or refused with the reason.
//! - [`Text`], [`window`], [`edit`]: the three over a file's text, pure.
//! - [`Lines`], [`Stamp`]: which lines a model has seen, and what a call
//!   leaves behind for read-first to ask (S21-Q20).
//! - [`run`]: the three against the file system, and [`Done`], whose text
//!   leaves only scrubbed; [`Job`], what `exec::Executor::file` runs (its
//!   seam, so a test cannot write a file through a tool).
//! - [`reading`]: a file call as the gate reads it — Claude Code's file
//!   tools are read through it too — of its path and of where that
//!   [`lands`] through a link (F2c).
//!
//! Offered to the model in every tool set (`ToolDef::offered`); the app
//! admits a call through its gate, its pane and its record
//! (`App::admit_file_call`). Held to read-first ([`Context`], F3); a secret
//! handle in what it writes asks, and is spent at the run
//! ([`Call::spent`], S21-Q27).

mod lands;
mod lines;
mod run;
mod text;

pub use lands::lands;
pub use lines::{Lines, Stamp};
pub use run::{Caps, Context, Done, Job, Scrub, Scrubbed, run};
pub use text::{Edited, Place, Text, Window, edit, window};

use serde_json::Value;
use std::path::{Path, PathBuf};

/// The tool that reads a file.
pub const READ: &str = "Read";
/// The tool that replaces text in a file.
pub const EDIT: &str = "Edit";
/// The tool that writes a whole file.
pub const WRITE: &str = "Write";

/// **A file tool's call, read from its input.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// `offset` is a 1-based line; `limit` how many from it, all when `None`.
    Read {
        path: PathBuf,
        offset: usize,
        limit: Option<usize>,
    },
    /// `old` replaced by `new`, once, or everywhere it is when `all`.
    Edit {
        path: PathBuf,
        old: String,
        new: String,
        all: bool,
    },
    /// The whole file, made or replaced.
    Write { path: PathBuf, content: String },
}

impl Call {
    /// **Read `name`'s call from `input`** — `None` when `name` is no file
    /// tool, else the call or why it is not one, said to the model as Claude
    /// Code's checks say it. A relative `file_path` is taken from `start`,
    /// the session's directory; Claude Code wants it absolute, Shannon takes
    /// both — and refuses a relative one when the start is unknown (`None`),
    /// rather than guess where it is. Numbers and switches may come as
    /// strings (`"5"`, `"true"`): a model sends them so, and they are taken.
    pub fn read(name: &str, input: &Value, start: Option<&Path>) -> Option<Result<Call, String>> {
        let keys: &[&str] = match name {
            READ => &["file_path", "offset", "limit"],
            EDIT => &["file_path", "old_string", "new_string", "replace_all"],
            WRITE => &["file_path", "content"],
            _ => return None,
        };
        Some(Self::read_as(name, keys, input, start))
    }

    fn read_as(
        name: &str,
        keys: &[&str],
        input: &Value,
        start: Option<&Path>,
    ) -> Result<Call, String> {
        let Some(fields) = input.as_object() else {
            return Err(format!("{name} takes an object: {}", keys.join(", ")));
        };
        // Refused, not ignored: a key the model believes does something (a
        // `pages`, a misspelt `limit`) should be told it does nothing.
        if let Some(key) = fields.keys().find(|key| !keys.contains(&key.as_str())) {
            return Err(format!(
                "{name} takes {}; `{key}` is not one",
                keys.join(", ")
            ));
        }
        let text = |key: &str| -> Result<String, String> {
            match &input[key] {
                Value::String(s) => Ok(s.clone()),
                Value::Null => Err(format!("{name} needs `{key}`")),
                _ => Err(format!("{name}'s `{key}` is text")),
            }
        };
        let path = path_from(&text("file_path")?, start)?;
        Ok(match name {
            READ => Call::Read {
                path,
                // 0 is read as the first line, as 1 is: a model may send
                // either for the start.
                offset: number(name, input, "offset")?.unwrap_or(1).max(1),
                limit: match number(name, input, "limit")? {
                    Some(0) => return Err(format!("{name}'s `limit` is at least 1")),
                    limit => limit,
                },
            },
            EDIT => Call::Edit {
                path,
                old: text("old_string")?,
                new: text("new_string")?,
                all: switch(name, input, "replace_all")?,
            },
            _ => Call::Write {
                path,
                content: text("content")?,
            },
        })
    }

    /// The file the call names.
    pub fn path(&self) -> &Path {
        match self {
            Call::Read { path, .. } | Call::Edit { path, .. } | Call::Write { path, .. } => path,
        }
    }

    /// What it does to its file, for the gate.
    pub fn act(&self) -> Act {
        match self {
            Call::Read { .. } => Act::Read,
            Call::Edit { .. } => Act::Edit,
            Call::Write { .. } => Act::Write,
        }
    }

    /// The tool's name.
    pub fn name(&self) -> &'static str {
        match self.act() {
            Act::Read => READ,
            Act::Edit => EDIT,
            Act::Write => WRITE,
        }
    }

    /// `Edit /work/a.rs` — the call in a few words, for a log line and an
    /// outcome's `command`.
    pub fn subject(&self) -> String {
        format!("{} {}", self.name(), self.path().display())
    }

    /// **The call as the gate judges it** — the shell line of its effect
    /// ([`reading`]) on the very file it opens: its path, joined to the start
    /// already, and where that path [`lands`] through a link, when that is
    /// another file. **Asks the file system.** Refused when it lands on a
    /// file whose name the rules cannot be told.
    ///
    /// **A secret it writes is said in the line** (S21-Q27): a stage
    /// `printf '%s' '{{h}}' > 'file'` — the value put into the file it
    /// opens — so the gate's scan of the line makes it the hard ask a
    /// command spending one is, and every reader of the line (the pane, a
    /// job's unattended check) sees the same spend.
    ///
    /// **So is the reach of a write to a file with more than one name**
    /// (review stop 16, E 3; Steven, 11-04: ask, and say why): a file
    /// written in place is changed under every name it has, and a hard link
    /// in the project can be the only name inside it — a package store's
    /// file under `.venv`, a dotfile linked in. Its other names cannot be
    /// listed, so the line says them as the shell would find them
    /// ([`reach`]), a word no rule can place: every reader of the line
    /// asks, and none can teach a rule that would not. The line is judged
    /// and shown, never run.
    pub fn judged(&self) -> Result<Judged, String> {
        let lands = lands(self.path())?;
        let into = lands.as_deref().unwrap_or(self.path());
        // **What is not a file is refused before anything opens it** (review
        // stop 16, E 6): an open alone acts on some devices — a serial
        // port's resets the board on it. A directory is the run's to refuse,
        // in each tool's words; opening one does nothing.
        if std::fs::metadata(into).is_ok_and(|meta| !meta.is_file() && !meta.is_dir()) {
            return Err(not_regular(into));
        }
        // `path_from` refused a path that is not text, and `lands` a landing
        // that is not: both are their text.
        let mut line = reading(
            self.act(),
            &self.path().to_string_lossy(),
            lands.as_deref().and_then(Path::to_str),
        );
        if let Some(reach) = reach(self.act(), into) {
            line = format!("{line}; {reach}");
        }
        let spends = self.spends();
        if !spends.is_empty() {
            let handles: Vec<String> = spends.iter().map(|h| crate::shell::quote(h)).collect();
            let into = crate::shell::quote(&into.to_string_lossy());
            line = format!("{line}; printf '%s' {} > {into}", handles.join(" "));
        }
        Ok(Judged { line, lands })
    }

    /// **The secret handles it would write** — a Write's content's, an
    /// Edit's `new_string`'s: each a value put into the file (S21-Q27). An
    /// `old_string`'s finds text by a handle — the file holds that value
    /// already — and writes nothing.
    pub fn spends(&self) -> Vec<&str> {
        match self {
            Call::Write { content, .. } => crate::secrets::secret_handles_in(content),
            Call::Edit { new, .. } => crate::secrets::secret_handles_in(new),
            Call::Read { .. } => Vec::new(),
        }
    }

    /// **The call with the values in** (S21-Q27, Steven: "as bash") — each
    /// secret handle in its text its value (`Aliases::put_in_file`), and the
    /// names of the secrets it spends ([`spends`](Self::spends)). At the run,
    /// after the gate and the pane saw the handles, as a command's values go
    /// in at the last moment. An Edit's `old_string` too, or it could not
    /// find the text a Read showed it by its handle; that one is not spent.
    /// Refused, said, for a handle not held or not lent.
    pub fn spent(&self, aliases: &crate::Aliases) -> Result<(Call, Vec<String>), String> {
        let mut call = self.clone();
        let spent = match &mut call {
            Call::Write { content, .. } => {
                let put = aliases.put_in_file(content)?;
                *content = put.text;
                put.spent
            }
            Call::Edit { old, new, .. } => {
                *old = aliases.put_in_file(old)?.text;
                let put = aliases.put_in_file(new)?;
                *new = put.text;
                put.spent
            }
            Call::Read { .. } => Vec::new(),
        };
        Ok((call, spent))
    }

    /// The same call, on `path`.
    fn at(&self, to: &Path) -> Call {
        let mut call = self.clone();
        match &mut call {
            Call::Read { path, .. } | Call::Edit { path, .. } | Call::Write { path, .. } => {
                *path = to.to_path_buf();
            }
        }
        call
    }
}

/// Why a device, a pipe or a socket is no file tool's: said the same at the
/// judgment ([`Call::judged`]) and at the run, for one put there since.
pub(crate) fn not_regular(path: &Path) -> String {
    format!(
        "{} is not a regular file — a device, a pipe or a socket; the file tools read and \
         write files",
        path.display()
    )
}

/// **How many names the file at `path` has, when more than one** — a hard
/// link: written in place, it changes under every name, wherever the others
/// are (review stop 16, E 3). `None` for one name, for nothing there, and
/// for what is not a regular file (a directory's count is its entries).
/// Through a link, of the file it lands on.
pub fn names(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    (meta.is_file() && meta.nlink() > 1).then(|| meta.nlink())
}

/// **What a write to `file` also writes, when it has several names** — every
/// name of the same file, as the shell would find them: a command
/// substitution, so the gate cannot place the word and asks whatever the
/// rules allow. A stage of the line, after the call's reading. `None` for a
/// read, and for a file of one name. Both backends' file calls consult it
/// ([`Call::judged`], `claude_code::reading`): whether Claude Code writes in
/// place is not known here, and an ask too many is the cheaper mistake.
pub fn reach(act: Act, file: &Path) -> Option<String> {
    if act == Act::Read {
        return None;
    }
    names(file)?;
    let path = crate::shell::quote(&file.to_string_lossy());
    Some(format!("cat > $(find / -samefile {path})"))
}

/// **A file call as the gate judges it** ([`Call::judged`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judged {
    /// The shell line judged: the call's reading of its path, and of where
    /// it lands.
    pub line: String,
    /// Where the path lands, when not on the file it names: the file the
    /// run opens (`Job::lands`).
    pub lands: Option<PathBuf>,
}

/// **What a file call does to its file**, as the gate weighs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Read,
    Write,
    /// Reads the file, then writes it.
    Edit,
}

/// **A file call as the gate reads it** — the shell line whose effect on the
/// file is the call's, so the rules, `[secrets] paths`, the inside/outside
/// default and `:analyze` judge it as they judge a command: a Read is
/// `cat 'p'`, a Write `cat > 'p'`, an Edit `cat 'p' > 'p'`. An Edit is a
/// read as well as a write: its answers say what the file holds ("found 3
/// matches"), so a file whose read is refused is not edited either.
///
/// The one door both backends' file calls pass (`claude_code::reading` for
/// Claude Code's, [`Call::judged`] for Shannon's own): one quoting
/// (`shell::quote`), one reading. `lands`: where the path [`lands`], when
/// that is another file — judged too ([`landed`]).
pub fn reading(act: Act, path: &str, lands: Option<&str>) -> String {
    let one = |path: &str| {
        let path = crate::shell::quote(path);
        match act {
            Act::Read => format!("cat {path}"),
            Act::Write => format!("cat > {path}"),
            Act::Edit => format!("cat {path} > {path}"),
        }
    };
    landed(one, path, lands)
}

/// **`line` of `path`, then of where it lands** — `cat 'P'; cat 'T'`: two
/// stages, each judged, the line as strict as the stricter (F2c). So a link
/// can only make a call ask where its path alone would not; never the
/// reverse. The one form a file call's landing takes on either backend.
pub fn landed(line: impl Fn(&str) -> String, path: &str, lands: Option<&str>) -> String {
    match lands {
        None => line(path),
        Some(at) => format!("{}; {}", line(path), line(at)),
    }
}

/// `file_path` as a path: absolute, or under `start`. `~` is refused, not
/// expanded: the gate reads the path as written, so a `~` Shannon expanded
/// would be a file the gate never judged, and one it did not would be a
/// directory named `~`.
fn path_from(written: &str, start: Option<&Path>) -> Result<PathBuf, String> {
    if written.is_empty() {
        return Err("`file_path` is empty".into());
    }
    if written == "~" || written.starts_with("~/") {
        return Err(format!(
            "`{written}`: `~` is not expanded here — write the path out in full"
        ));
    }
    let path = Path::new(written);
    let path = match (path.is_absolute(), start) {
        (true, _) => path.to_path_buf(),
        (false, Some(start)) => start.join(path),
        (false, None) => {
            return Err(format!(
                "`{written}` is relative, and the working directory is unknown — give the \
                 path in full"
            ));
        }
    };
    // The gate reads the path as text ([`reading`]): one that is not text
    // would be judged as a lossy spelling of another file.
    match path.to_str() {
        Some(_) => Ok(path),
        None => Err(format!(
            "`{written}` is not a path Shannon can name to its rules (it is not text)"
        )),
    }
}

/// A whole number at `key`, as a number or a string of digits; `None` when
/// absent.
fn number(name: &str, input: &Value, key: &str) -> Result<Option<usize>, String> {
    let refuse = || format!("{name}'s `{key}` is a whole number of lines, 0 or more");
    match &input[key] {
        Value::Null => Ok(None),
        Value::Number(n) => n.as_u64().map(|n| Some(n as usize)).ok_or_else(refuse),
        Value::String(s) => s.trim().parse().map(Some).map_err(|_| refuse()),
        _ => Err(refuse()),
    }
}

/// A switch at `key`, as a boolean or `"true"`/`"false"`; off when absent.
fn switch(name: &str, input: &Value, key: &str) -> Result<bool, String> {
    match &input[key] {
        Value::Null => Ok(false),
        Value::Bool(on) => Ok(*on),
        Value::String(s) if s == "true" => Ok(true),
        Value::String(s) if s == "false" => Ok(false),
        _ => Err(format!("{name}'s `{key}` is true or false")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn read(name: &str, input: Value) -> Result<Call, String> {
        Call::read(name, &input, Some(Path::new("/work"))).expect("a file tool")
    }

    /// **Claude Code's inputs read as its tools read them** — a relative
    /// path under the start, offset 0 as line 1, numbers and switches as
    /// strings — and nothing else is a file tool.
    #[test]
    fn a_call_reads_as_claude_codes_tools_take_it() {
        assert_eq!(
            read(READ, json!({"file_path": "src/a.rs"})),
            Ok(Call::Read {
                path: "/work/src/a.rs".into(),
                offset: 1,
                limit: None
            })
        );
        assert_eq!(
            read(
                READ,
                json!({"file_path": "/x/a", "offset": 0, "limit": "60"})
            ),
            Ok(Call::Read {
                path: "/x/a".into(),
                offset: 1,
                limit: Some(60)
            })
        );
        assert_eq!(
            read(
                EDIT,
                json!({"file_path": "a", "old_string": "x", "new_string": "", "replace_all": "true"})
            ),
            Ok(Call::Edit {
                path: "/work/a".into(),
                old: "x".into(),
                new: String::new(),
                all: true
            })
        );
        assert_eq!(
            read(WRITE, json!({"file_path": "a", "content": "hi\n"})),
            Ok(Call::Write {
                path: "/work/a".into(),
                content: "hi\n".into()
            })
        );
        assert!(Call::read("bash", &json!({"command": "ls"}), Some(Path::new("/"))).is_none());
        // With no start known, a relative path is refused, an absolute one read.
        let unplaced = |path: &str| Call::read(READ, &json!({ "file_path": path }), None).unwrap();
        assert!(
            unplaced("a.rs")
                .unwrap_err()
                .contains("working directory is unknown")
        );
        assert!(unplaced("/x/a.rs").is_ok());
    }

    /// **A call is judged as the shell line with its effect** — of the very
    /// file it opens (the start joined), its path one quoted word, and an
    /// Edit as a read and a write.
    #[test]
    fn a_call_reads_to_the_gate_as_its_effect() {
        let reads = |name: &str, input: Value| read(name, input).unwrap().judged().unwrap().line;
        assert_eq!(reads(READ, json!({"file_path": "a b"})), "cat '/work/a b'");
        assert_eq!(
            reads(WRITE, json!({"file_path": "/x/it's", "content": ""})),
            r"cat > '/x/it'\''s'"
        );
        assert_eq!(
            reads(
                EDIT,
                json!({"file_path": "a", "old_string": "x", "new_string": "y"})
            ),
            "cat '/work/a' > '/work/a'"
        );
        assert_eq!(
            read(READ, json!({"file_path": "a"})).unwrap().subject(),
            "Read /work/a"
        );
    }

    /// **The handle an Edit writes is said in its line; one it finds text
    /// by is not** (review stop 16, E 2: `spends`' Edit arm emptied, every
    /// suite passed, and the edit wrote the value unasked). `new_string`'s
    /// puts a value in the file — the stage the gate makes a hard ask;
    /// `old_string`'s finds text the file already holds.
    #[test]
    fn an_edit_says_the_handle_it_writes_not_the_one_it_finds_by() {
        let edit = |old: &str, new: &str| {
            let input = json!({"file_path": "a", "old_string": old, "new_string": new});
            read(EDIT, input).unwrap().judged().unwrap().line
        };
        assert_eq!(
            edit("x", "K={{db.4aa4}}"),
            "cat '/work/a' > '/work/a'; printf '%s' '{{db.4aa4}}' > '/work/a'"
        );
        assert_eq!(edit("K={{db.4aa4}}", "x"), "cat '/work/a' > '/work/a'");
    }

    /// **A file of two names asks before it is written, whatever the rules
    /// allow** (review stop 16, E 3, measured: an Edit of `proj/rc`, hard
    /// linked to `home/rc`, ran unasked as an inside write and changed
    /// `home/rc`). Its line says the write's reach; the gate asks of it
    /// under the shipped rules and under blanket allows, and no rule can
    /// answer it. A Read of it, and a write of a file of one name, are
    /// judged as ever.
    #[test]
    fn a_file_of_two_names_asks_before_it_is_written() {
        use crate::permission::{Decision, NoProposal, RuleSet};
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("theirs"), "x\n").unwrap();
        std::fs::hard_link(d.join("theirs"), d.join("rc")).unwrap();
        std::fs::write(d.join("plain"), "x\n").unwrap();
        let judged = |name: &str, input: Value| {
            let call = Call::read(name, &input, Some(d)).unwrap().unwrap();
            call.judged().unwrap().line
        };
        let rc = d.join("rc").display().to_string();
        let reach = format!("cat > $(find / -samefile '{rc}')");
        let edit = judged(
            EDIT,
            json!({"file_path": "rc", "old_string": "x", "new_string": "y"}),
        );
        assert_eq!(edit, format!("cat '{rc}' > '{rc}'; {reach}"));
        let write = judged(WRITE, json!({"file_path": "rc", "content": "y"}));
        assert_eq!(write, format!("cat > '{rc}'; {reach}"));
        assert_eq!(
            judged(READ, json!({"file_path": "rc"})),
            format!("cat '{rc}'")
        );
        let plain = d.join("plain").display().to_string();
        assert_eq!(
            judged(WRITE, json!({"file_path": "plain", "content": "y"})),
            format!("cat > '{plain}'")
        );
        assert_eq!(names(&d.join("rc")), Some(2));
        assert_eq!(names(d), None, "a directory's count is its entries");

        let shipped = RuleSet::defaults().with_start(&d.display().to_string());
        let mut wide = shipped.clone();
        wide.extend_from_lines("allow :* /\nallow *").unwrap();
        for (rules, named) in [(&shipped, "shipped"), (&wide, "blanket allows")] {
            assert_eq!(
                rules.decide_line(&plain_write(&plain)).decision,
                Decision::Allow,
                "premise, under {named}: an inside write runs unasked"
            );
            for line in [&edit, &write] {
                assert_eq!(
                    rules.decide_line(line).decision,
                    Decision::Ask,
                    "{named}: {line}"
                );
                assert!(
                    matches!(
                        rules.proposed_allow(line),
                        Err(NoProposal::RuleCannotAnswer(_))
                    ),
                    "{named}: {line}: {:?}",
                    rules.proposed_allow(line)
                );
            }
        }
    }

    /// `cat > 'path'`, a write of one name.
    fn plain_write(path: &str) -> String {
        format!("cat > '{path}'")
    }

    /// **What is not a file is refused at the judgment, before any open**
    /// (review stop 16, E 6: an open alone acts on some devices) — a device,
    /// for every tool, said as the run says it; a directory is left to the
    /// run, which says what to do in each tool's words.
    #[test]
    fn what_is_not_a_file_is_refused_before_it_is_opened() {
        for (name, input) in [
            (READ, json!({"file_path": "/dev/null"})),
            (WRITE, json!({"file_path": "/dev/null", "content": ""})),
            (
                EDIT,
                json!({"file_path": "/dev/null", "old_string": "a", "new_string": "b"}),
            ),
        ] {
            let said = read(name, input).unwrap().judged().unwrap_err();
            assert_eq!(said, not_regular(Path::new("/dev/null")), "{name}");
        }
        let dir = tempfile::tempdir().unwrap();
        let input = json!({"file_path": dir.path().to_str().unwrap()});
        assert!(read(READ, input).unwrap().judged().is_ok());
    }

    /// **A call through a link is judged on its path and where it lands** —
    /// each a stage of one line, for every act; and the landing is what the
    /// run opens.
    #[test]
    fn a_call_through_a_link_is_judged_where_it_lands() {
        for (act, line) in [
            (Act::Read, "cat '/p/n'; cat '/q/s'"),
            (Act::Write, "cat > '/p/n'; cat > '/q/s'"),
            (Act::Edit, "cat '/p/n' > '/p/n'; cat '/q/s' > '/q/s'"),
        ] {
            assert_eq!(reading(act, "/p/n", Some("/q/s")), line);
        }
        // A landing is where the file really is; the path as it was given
        // (the temp directory's own root link, `/tmp` on macOS, kept).
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_path_buf();
        let real = std::fs::canonicalize(&d).unwrap();
        std::fs::write(d.join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(d.join("secret"), d.join("notes")).unwrap();
        let call = Call::read(READ, &json!({"file_path": "notes"}), Some(&d))
            .unwrap()
            .unwrap();
        let judged = call.judged().unwrap();
        assert_eq!(judged.lands, Some(real.join("secret")));
        assert_eq!(
            judged.line,
            format!(
                "cat '{}'; cat '{}'",
                d.join("notes").display(),
                real.join("secret").display()
            )
        );
    }

    /// **What is not a call is refused, and says why** — each a mistake a
    /// model makes, told rather than guessed at.
    #[test]
    fn what_is_not_a_call_says_why() {
        let cases = [
            (READ, json!("a.rs"), "takes an object"),
            (READ, json!({}), "needs `file_path`"),
            (READ, json!({"file_path": ""}), "is empty"),
            (READ, json!({"file_path": "~/a"}), "`~` is not expanded"),
            (
                READ,
                json!({"file_path": "a", "pages": "1-5"}),
                "`pages` is not one",
            ),
            (READ, json!({"file_path": "a", "limit": 0}), "at least 1"),
            (
                READ,
                json!({"file_path": "a", "offset": -3}),
                "whole number",
            ),
            (READ, json!({"file_path": 7}), "is text"),
            (
                EDIT,
                json!({"file_path": "a", "old_string": "x"}),
                "needs `new_string`",
            ),
            (
                EDIT,
                json!({"file_path": "a", "old_string": "x", "new_string": "y", "replace_all": 1}),
                "true or false",
            ),
            (WRITE, json!({"file_path": "a"}), "needs `content`"),
        ];
        for (name, input, why) in cases {
            let said = read(name, input.clone()).expect_err(&format!("{input} refused"));
            assert!(said.contains(why), "{input}: {said}");
        }
        // A start whose name is not text: the gate would judge a lossy
        // spelling of the file opened (review stop 16, B 2).
        use std::os::unix::ffi::OsStrExt;
        let start = Path::new(std::ffi::OsStr::from_bytes(b"/w\xff"));
        let said = Call::read(READ, &json!({"file_path": "a"}), Some(start))
            .expect("a file tool")
            .expect_err("refused");
        assert!(said.contains("it is not text"), "{said}");
    }
}
