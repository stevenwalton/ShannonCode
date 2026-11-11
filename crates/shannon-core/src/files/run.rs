//! **The three against the file system** — what a file call does to your
//! disk, and the [`Done`] it answers with, whose text leaves only through
//! [`Done::scrub`].
//!
//! A file is written **in place**: opened, truncated, written. That is what
//! `cat > path` does, and `cat > path` is what the gate judges a write as
//! (`file-tools.md` §3.3). Temp-and-rename would replace a symlink with a
//! file, drop hard links and reset the mode and owner — an act the gate
//! never saw. A new file takes your umask, as a shell redirect's does: it
//! is a file of your project, not one of Shannon's own. No directory is
//! made: `cat > a/b` makes none either, and a `mkdir` is a call the gate
//! sees.

use super::Call;
use super::lines::{Lines, Stamp, seen_now};
use super::text::{self, Place, Text};
use crate::config::ReadFirst;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// **How much a file tool reads and shows**: 2,000 lines and 256 KiB a
/// Read, a line cut at 2,000 characters — Shannon's starting point, and a
/// setting (Claude Code's tools reference documents only its own Read
/// refusing a whole file over 256 KB, read in pages). `max_file` bounds
/// what is read into memory at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caps {
    pub max_lines: usize,
    pub max_bytes: usize,
    pub line_chars: usize,
    pub max_file: u64,
}

impl Default for Caps {
    fn default() -> Self {
        Caps {
            max_lines: 2000,
            max_bytes: 256 * 1024,
            line_chars: 2000,
            max_file: 64 * 1024 * 1024,
        }
    }
}

/// **What the model's context holds of a file, as a call on it runs** (F3,
/// `file-tools.md` §3.6): the stamps of the calls on it whose results are
/// still in the conversation it was sent — not a side cache, so a fork, an
/// edit of history or a compaction changes what was read — and what a
/// change asks of them (`[files] read_first`). `Default`: nothing read, a
/// change asking for some of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    pub stamps: Vec<Stamp>,
    pub first: ReadFirst,
}

impl Context {
    /// **A call refused before anything is asked** — an Edit of a file
    /// nothing in the context has read, under `[files] read_first`: known
    /// without opening the file, so the model is told before a pane asks
    /// you about a change that could not run. Whether what was read is
    /// still the file, and a Write over one that exists, are the run's to
    /// check ([`run`]) — the disk's to say, and not on the UI's thread.
    pub fn unread(&self, call: &Call) -> Option<String> {
        let edit = matches!(call, Call::Edit { .. });
        let asks = self.first != ReadFirst::Off && edit;
        (asks && self.stamps.is_empty()).then(|| not_read(call.path()))
    }

    /// **What the model has seen of the file, if a change to it may go
    /// ahead** — a file of `total` lines whose bytes are `bytes`, under
    /// `[files] read_first`; else what it must do first. Checked where the
    /// file is read to be changed, so a change by anyone since the read —
    /// you, a formatter, a `bash` command — is caught at the run.
    fn seen(&self, path: &Path, bytes: &[u8], total: usize) -> Result<Lines, String> {
        let shown = path.display();
        match (self.first, seen_now(&self.stamps, bytes)) {
            (ReadFirst::Off, seen) => Ok(seen.unwrap_or_default()),
            (_, None) if self.stamps.is_empty() => Err(not_read(path)),
            (_, None) => Err(format!(
                "{shown} has changed since you read it — Read it again first"
            )),
            (ReadFirst::Whole, Some(seen)) if !seen.covers(total) => Err(format!(
                "{shown} has been read only in part, and every line must be (`[files] \
                 read_first = \"whole\"`) — Read the rest first"
            )),
            (_, Some(seen)) => Ok(seen),
        }
    }
}

/// What the model is told of a change to a file it has not read.
fn not_read(path: &Path) -> String {
    format!(
        "{} has not been read — Read it first, then change it",
        path.display()
    )
}

/// **A file call to run**, with what it is run under: where its path lands
/// through a link, what the model's context holds of its file
/// ([`Context`]), and the caps. What `exec::Executor::file` is handed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub call: Call,
    /// Where the path lands, when not on the file it names (F2c,
    /// `Judged::lands`): the file judged, opened by that name, so a link
    /// changed since the judgment cannot move the call to another.
    pub lands: Option<PathBuf>,
    pub context: Context,
    pub caps: Caps,
}

impl Job {
    /// **Run it** ([`run`]) — at its landing when it has one, and saying so,
    /// to the model and on the record: what was read or changed is that file.
    pub fn run(&self, scrub: Scrub) -> Done {
        let Some(at) = &self.lands else {
            return run(&self.call, &self.caps, &self.context, scrub);
        };
        let mut done = run(&self.call.at(at), &self.caps, &self.context, scrub);
        done.text.push_str(&format!(
            "\n[through a link: {} is {}]",
            self.call.path().display(),
            at.display()
        ));
        done
    }

    /// **The file it opens** — its landing, or the path it names: what its
    /// stamp names, and what a later call's context is looked up by.
    pub fn file(&self) -> &Path {
        self.lands.as_deref().unwrap_or(self.call.path())
    }

    /// `Read /work/notes`, or `Read /work/notes → /elsewhere/secret` through
    /// a link — the call in a few words, for a log line and an outcome.
    pub fn subject(&self) -> String {
        match &self.lands {
            None => self.call.subject(),
            Some(at) => format!("{} → {}", self.call.subject(), at.display()),
        }
    }
}

/// **What a file call's text is scrubbed by** — what a command's output
/// passes (`secrets::redact`): your `[secrets]` policy and the values
/// Shannon holds. A Read's file is scrubbed by it before a line is numbered
/// or cut (`read`); every call's text, again, at the door
/// ([`Done::scrub`]) — redaction is idempotent, so the second pass changes
/// nothing the first made.
#[derive(Debug, Clone, Copy)]
pub struct Scrub<'a> {
    pub secrets: &'a crate::SecretsConfig,
    pub aliases: &'a crate::Aliases,
}

/// **What a file call did.** Its text — a Read's is the file — is private:
/// it leaves only through [`scrub`](Self::scrub), the redaction `bash`'s
/// output passes (`exec`), so no caller can send a file unscrubbed.
#[derive(Debug)]
pub struct Done {
    text: String,
    /// What a Read's own scrub took out of the lines it shows ([`read`]):
    /// the door finds nothing more there, so its count would say none.
    taken: Taken,
    /// Refused, or failed: the model is told it as an error.
    pub is_error: bool,
    /// What read-first keeps of the call: `None` when it touched no file.
    pub stamp: Option<Stamp>,
    /// Where an Edit's replacements fell, for the record's gutter.
    pub places: Vec<Place>,
}

/// What a scrub took: credential kinds removed, held values masked —
/// [`Scrubbed`]'s two lists.
#[derive(Debug, Default)]
struct Taken {
    redacted: Vec<String>,
    masked: Vec<String>,
}

/// [`Done`], its text scrubbed and safe to send and keep.
#[derive(Debug)]
pub struct Scrubbed {
    pub text: String,
    /// Credential kinds removed one way, as `ExecOutcome::redacted`.
    pub redacted: Vec<String>,
    /// Held secrets shown as their handle.
    pub masked: Vec<String>,
    pub is_error: bool,
    pub stamp: Option<Stamp>,
    pub places: Vec<Place>,
}

impl Done {
    /// Refused, or failed: nothing done, `why` the model's to read.
    pub(crate) fn refused(why: String) -> Done {
        Done {
            text: why,
            taken: Taken::default(),
            is_error: true,
            stamp: None,
            places: Vec::new(),
        }
    }

    fn did(text: String, stamp: Stamp, places: Vec<Place>) -> Done {
        Done {
            text,
            taken: Taken::default(),
            is_error: false,
            stamp: Some(stamp),
            places,
        }
    }

    /// **The text, scrubbed** as a command's output is
    /// (`secrets::redact`): credentials removed, held secrets as handles —
    /// and saying what a Read's own scrub took, too.
    pub fn scrub(self, scrub: Scrub) -> Scrubbed {
        let redaction = crate::secrets::redact(&self.text, scrub.secrets, scrub.aliases);
        let Taken {
            mut redacted,
            mut masked,
        } = self.taken;
        redacted.extend(redaction.kinds);
        for name in redaction.masked {
            if !masked.contains(&name) {
                masked.push(name);
            }
        }
        Scrubbed {
            text: redaction.text,
            redacted,
            masked,
            is_error: self.is_error,
            stamp: self.stamp,
            places: self.places,
        }
    }
}

/// **Run `call`**, under what the model's context holds of its file
/// (`context`): an Edit, or a Write over a text file, goes ahead only as
/// `[files] read_first` allows, and carries what was seen forward; a Read
/// of lines it was shown, the file unchanged, is answered by pointing back.
/// Whether the file still hashes as a stamp saw it is checked here, at the
/// run, not trusted. A Read's file is scrubbed by `scrub` before it is
/// numbered.
pub fn run(call: &Call, caps: &Caps, context: &Context, scrub: Scrub) -> Done {
    let done = match call {
        Call::Read {
            path,
            offset,
            limit,
        } => read(path, *offset, *limit, caps, context, scrub),
        Call::Edit {
            path,
            old,
            new,
            all,
        } => edit(path, old, new, *all, caps, context),
        Call::Write { path, content } => write(path, content, caps, context, scrub),
    };
    done.unwrap_or_else(Done::refused)
}

fn read(
    path: &Path,
    offset: usize,
    limit: Option<usize>,
    caps: &Caps,
    context: &Context,
    scrub: Scrub,
) -> Result<Done, String> {
    let (_, bytes) = there(path, Open::Read, caps)?;
    let text = decoded(path, &bytes)?;
    let (window, taken) = shown(&text, offset, limit, caps, scrub);
    let (first, last) = (window.first, window.last);
    let stamp = Stamp::read(path.to_path_buf(), &bytes, window.total, first, last);
    // **An unchanged re-read points back**: lines it was shown, the file as
    // it was, are still in its context — the whole read again is tokens for
    // nothing.
    let shown: Vec<Stamp> = (context.stamps.iter())
        .filter(|stamp| stamp.shown)
        .cloned()
        .collect();
    let still = seen_now(&shown, &bytes).is_some_and(|seen| seen.holds(first, last));
    if still && first <= last {
        let said = format!(
            "{} is unchanged since you read {} {} of it above — that result is still current; \
             refer to it rather than reading it again.",
            path.display(),
            if first == last { "line" } else { "lines" },
            text::span(first, last + 1 - first)
        );
        return Ok(Done::did(said, stamp, Vec::new()));
    }
    Ok(Done {
        taken,
        ..Done::did(window.report(), stamp, Vec::new())
    })
}

/// **What a Read of `text` shows, and what was taken out of it** —
/// scrubbed before it is numbered or cut: a held value that ran over lines,
/// or one the cut fell inside, was handed to the model when the scrub came
/// after (review stop 16, E 1).
///
/// Where a replacement could run over a line (`secrets::crosses_lines`: a
/// value held or configured with a newline, a key block, a URL's userinfo
/// broken by one), the file is scrubbed whole, its lines kept
/// (`secrets::redact_lines`) — whole, because a value can begin above the
/// window and end inside it. Anywhere else every replacement falls within
/// a line, so each line shown is scrubbed alone, whole, then cut: the cost
/// of what is shown, not of the file (scrubbed whole, a 16 MiB file's Read
/// took 2.1 s, its window 11 ms; measured). What it took is counted over
/// the lines shown alone (`secrets::added_redactions`) — a key on line 500
/// is not news to a Read of lines 1–100.
fn shown(
    text: &Text,
    offset: usize,
    limit: Option<usize>,
    caps: &Caps,
    scrub: Scrub,
) -> (text::Window, Taken) {
    let (secrets, aliases) = (scrub.secrets, scrub.aliases);
    let mut masked = Vec::new();
    let window = match crate::secrets::crosses_lines(text.body(), secrets, aliases) {
        true => {
            let clean = crate::secrets::redact_lines(text.body(), secrets, aliases);
            masked = clean.masked;
            text::window(&clean.text, offset, limit, caps, str::to_string)
        }
        false => text::window(text.body(), offset, limit, caps, |line| {
            let clean = crate::secrets::redact_lines(line, secrets, aliases);
            for name in clean.masked {
                if !masked.contains(&name) {
                    masked.push(name);
                }
            }
            clean.text
        }),
    };
    let raw = lines_of(text.body(), window.first, window.last);
    let taken = Taken {
        redacted: crate::secrets::added_redactions(raw, &window.shown),
        masked,
    };
    (window, taken)
}

/// Lines `first..=last` of `body` (from 1), as one slice of it.
fn lines_of(body: &str, first: usize, last: usize) -> &str {
    if first > last {
        return "";
    }
    let (mut at, mut from) = (0, body.len());
    for (n, line) in (1..).zip(body.split_inclusive('\n')) {
        if n == first {
            from = at;
        }
        at += line.len();
        if n == last {
            break;
        }
    }
    &body[from.min(at)..at]
}

fn edit(
    path: &Path,
    old: &str,
    new: &str,
    all: bool,
    caps: &Caps,
    context: &Context,
) -> Result<Done, String> {
    let (mut file, bytes) = there(path, Open::Change, caps)?;
    let text = decoded(path, &bytes)?;
    let seen = context.seen(path, &bytes, text.lines())?;
    let edited = text::edit(&text, old, new, all)?;
    let out = edited.text.encode();
    rewrite(&mut file, path, &out, &bytes)?;
    let stamp = Stamp::edited(
        seen,
        path.to_path_buf(),
        &edited.places,
        &out,
        edited.text.lines(),
    );
    Ok(Done::did(
        edited.report(&path.display().to_string()),
        stamp,
        edited.places,
    ))
}

fn write(
    path: &Path,
    content: &str,
    caps: &Caps,
    context: &Context,
    scrub: Scrub,
) -> Result<Done, String> {
    let shown = path.display();
    // The file there now, if any — opened once ([`open`]): its lines for
    // the report, its line endings and mark for what replaces it, the
    // descriptor it is written through, and its bytes, put back if the
    // write fails. One that is not text is replaced as written, and asks no
    // read first: none could show it.
    let mut was = match open(path, Open::Replace, caps)? {
        Some((file, bytes)) => {
            let text = Text::decode(&bytes).ok();
            if let Some(text) = &text {
                context.seen(path, &bytes, text.lines())?;
            }
            Some((file, bytes, text))
        }
        None => None,
    };
    if was.is_none()
        && let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
        && !parent.is_dir()
    {
        return Err(format!(
            "There is no directory {} — make it first (`mkdir -p`)",
            parent.display()
        ));
    }
    let out = match &was {
        Some((_, _, Some(text))) => text.like(content).encode(),
        _ => content.as_bytes().to_vec(),
    };
    match &mut was {
        Some((file, old, _)) => rewrite(file, path, &out, old)?,
        // A new file holding a value you marked secret is yours alone —
        // the content as it runs, its handles spent (`Call::spent`).
        None => {
            let (_, secrets) = scrub.aliases.mask_secrets(content);
            create(path, &out, !secrets.is_empty())?
        }
    }
    let lines = content.lines().count();
    let made = text::counted(lines, "line");
    let report = match was {
        None => format!("Created {shown} ({made})."),
        Some((_, _, Some(text))) => format!("Wrote {shown} ({made}, was {}).", text.lines()),
        Some((_, _, None)) => format!("Wrote {shown} ({made}; it was not text)."),
    };
    let stamp = Stamp::written(path.to_path_buf(), &out, lines);
    Ok(Done::did(report, stamp, Vec::new()))
}

/// **How a call opens its file** — what it may do there, and so what it is
/// told when it cannot: each tool's own verb and advice, through one
/// [`open`]. Three spellings of it disagreed (review stop 16, D 2): an
/// Edit of a directory was told the raw "Is a directory (os error 21)",
/// and an Edit of a file too big Read's advice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    /// A Read: the file must be there.
    Read,
    /// An Edit: there, and written back through the same descriptor.
    Change,
    /// A Write: there and rewritten, or missing and made ([`create`]).
    Replace,
}

impl Open {
    /// The verb a refusal names.
    fn verb(self) -> &'static str {
        match self {
            Open::Read => "read",
            Open::Change => "change",
            Open::Replace => "write",
        }
    }

    /// What to use instead of a file tool for a file too big for one.
    fn too_big(self) -> &'static str {
        match self {
            Open::Read => "read part of it with bash (`sed -n`, `head`)",
            Open::Change => "change it with bash (`sed -i`)",
            Open::Replace => "replace it with bash (`cat > file`)",
        }
    }

    /// What to do instead when the path is a directory.
    fn directory(self) -> &'static str {
        match self {
            Open::Read | Open::Change => "list it with `ls`",
            Open::Replace => "name a file in it",
        }
    }
}

/// **The file, opened once for the whole call, and its bytes** — or `None`
/// where a Write finds none to replace. Its read and its write go through
/// this one descriptor, so nothing done to the path between them moves the
/// write onto another file (review stop 16, B 6: the path was opened twice,
/// and resolved twice).
///
/// **Opened as judged** (E 5): by the name the call was judged at — its
/// landing, through a link (`Job::lands`) — and not through a link at its
/// last step, so a link put there since the judgment is refused, never
/// written through (measured: a Write judged with nothing at `proj/b`
/// wrote `home/blob` through a link planted after). A directory above it
/// swapped for a link is not caught here; Shannon is no sandbox, and a
/// process that can race it can write the file itself.
///
/// Opened without blocking: a FIFO opened to read waited for a writer,
/// forever, before anything could ask what it was (B 1). Kept for the
/// reads and writes after it too: on Linux some regular files block (a
/// trace pipe, the kernel's log), and those fail at once instead (E 7).
/// Never as a terminal to control (E 6). What it is, the descriptor's own
/// `fstat` says, not a list of names: a device, a pipe or a socket reports a
/// size of 0, so the cap never applied and `/dev/zero` read until memory ran
/// out. The judgment refuses those before any open (`Call::judged`); this is
/// for one put there since. The read itself stops past the cap, for a file
/// whose size is misreported (procfs). Refused, each saying what to do
/// instead in `how`'s words.
fn open(path: &Path, how: Open, caps: &Caps) -> Result<Option<(std::fs::File, Vec<u8>)>, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let shown = path.display();
    let directory = || format!("{shown} is a directory — {}", how.directory());
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .write(how != Open::Read)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_NOCTTY)
        .open(path);
    let mut file = match opened {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound && how == Open::Replace => return Ok(None),
        // A directory refuses a writable open before `fstat` can be asked.
        Err(e) if e.kind() == ErrorKind::IsADirectory => return Err(directory()),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(format!(
                "{shown} is a link now, and was not when this call was judged — it was not \
                 opened; ask for it again"
            ));
        }
        Err(e) => return Err(failed(how.verb(), path, &e)),
    };
    let meta = file.metadata().map_err(|e| failed(how.verb(), path, &e))?;
    if meta.is_dir() {
        return Err(directory());
    }
    if !meta.is_file() {
        return Err(super::not_regular(path));
    }
    let too_big = |size: u64| {
        format!(
            "{shown} is {size} bytes, more than a file tool takes ({}) — {}",
            caps.max_file,
            how.too_big()
        )
    };
    if meta.len() > caps.max_file {
        return Err(too_big(meta.len()));
    }
    let bytes = capped(&mut file, meta.len(), caps.max_file)
        .map_err(|e| failed(how.verb(), path, &e))?
        .map_err(too_big)?;
    Ok(Some((file, bytes)))
}

/// **At most `cap` bytes of `from`**, or how many it held past them — read
/// to one byte beyond, so a file whose size was misreported (procfs: `size`
/// as `0`) is refused rather than read until memory runs out, or cut and
/// written back short (E 8: deleting the check passed every test).
fn capped(
    from: &mut impl std::io::Read,
    size: u64,
    cap: u64,
) -> std::io::Result<Result<Vec<u8>, u64>> {
    use std::io::Read as _;
    let mut bytes = Vec::with_capacity(size.min(cap) as usize);
    from.take(cap + 1).read_to_end(&mut bytes)?;
    Ok(match bytes.len() as u64 > cap {
        true => Err(bytes.len() as u64),
        false => Ok(bytes),
    })
}

/// [`open`] for a call whose file must be there: a Read's or an Edit's.
/// (`open` refuses a missing one itself for these; this is its type.)
fn there(path: &Path, how: Open, caps: &Caps) -> Result<(std::fs::File, Vec<u8>), String> {
    (open(path, how, caps)?)
        .ok_or_else(|| failed(how.verb(), path, &std::io::Error::from(ErrorKind::NotFound)))
}

/// `bytes` as text, or what to use instead.
fn decoded(path: &Path, bytes: &[u8]) -> Result<Text, String> {
    Text::decode(bytes).map_err(|why| {
        format!(
            "{} {why} — look at it with bash (`file`, `xxd`)",
            path.display()
        )
    })
}

/// **What a file is rewritten through** — the descriptor that was read, or
/// in a test a disk that fills where it is told to (E 7).
trait Body: std::io::Write + std::io::Seek {
    fn set_len(&mut self, len: u64) -> std::io::Result<()>;
}

impl Body for std::fs::File {
    fn set_len(&mut self, len: u64) -> std::io::Result<()> {
        std::fs::File::set_len(self, len)
    }
}

/// **Written over, then cut to its length, through the descriptor that was
/// read** — what `cat > path` does (the module doc), on the very file
/// judged and read, in the order that loses least when a write fails: cut
/// first, a full disk left the file empty and the model was told only
/// "Cannot write" (review stop 16, E 7). A write that fails puts back what
/// was there, `was`, as far as it can, and says whether it could.
fn rewrite(file: &mut impl Body, path: &Path, bytes: &[u8], was: &[u8]) -> Result<(), String> {
    let Err(e) = put(file, bytes) else {
        return Ok(());
    };
    let back = match put(file, was) {
        Ok(()) => "its text was put back as it was",
        Err(_) => "it may be part-written — read it before trusting it",
    };
    Err(format!("{} — {back}", failed("write", path, &e)))
}

/// `bytes` over `file` from its start, and the file cut to their length.
fn put(file: &mut impl Body, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::SeekFrom;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(bytes)?;
    file.set_len(bytes.len() as u64)
}

/// **A file made new** — `O_EXCL`: one that appeared at `path` since it was
/// found missing (a link among them) refuses, never is written through.
/// Your umask's mode, as a redirect's — or owner-only when `private`: a
/// file a held secret was written into (review stop 16, E 10).
fn create(path: &Path, bytes: &[u8], private: bool) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(if private { 0o600 } else { 0o666 })
        .open(path)
        .map_err(|e| match e.kind() {
            // Not found to open, yet there: a link to nothing, or a file
            // made since.
            ErrorKind::AlreadyExists => match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.file_type().is_symlink() => format!(
                    "{} is a link to a file that does not exist — write the file it names",
                    path.display()
                ),
                _ => format!(
                    "{} appeared while this call ran — read it, then decide again",
                    path.display()
                ),
            },
            _ => failed("write", path, &e),
        })?;
    file.write_all(bytes).map_err(|e| failed("write", path, &e))
}

/// What the model is told when the file system said no.
fn failed(doing: &str, path: &Path, e: &std::io::Error) -> String {
    let shown = path.display();
    match e.kind() {
        ErrorKind::NotFound => format!("File does not exist: {shown}"),
        ErrorKind::PermissionDenied => format!("Cannot {doing} {shown}: permission denied"),
        _ => format!("Cannot {doing} {shown}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scrub a default config gives, nothing held: what a test whose
    /// subject is not secrets runs under.
    fn plain() -> Scrub<'static> {
        static SECRETS: std::sync::LazyLock<crate::SecretsConfig> =
            std::sync::LazyLock::new(Default::default);
        static ALIASES: std::sync::LazyLock<crate::Aliases> =
            std::sync::LazyLock::new(Default::default);
        Scrub {
            secrets: &SECRETS,
            aliases: &ALIASES,
        }
    }

    /// [`run`](super::run) under [`plain`] — every test here but the
    /// secrets' own.
    fn run(call: &Call, caps: &Caps, context: &Context) -> Done {
        super::run(call, caps, context, plain())
    }

    fn scrubbed(done: Done) -> Scrubbed {
        done.scrub(plain())
    }

    /// A context asking nothing read first — for a test whose subject is
    /// not read-first.
    fn unasked() -> Context {
        Context {
            stamps: Vec::new(),
            first: ReadFirst::Off,
        }
    }

    /// A context holding `stamps`, under `first`.
    fn holding(stamps: &[&Stamp], first: ReadFirst) -> Context {
        let stamps = stamps.iter().map(|&stamp| stamp.clone()).collect();
        Context { stamps, first }
    }

    fn call_read(path: &Path) -> Call {
        Call::Read {
            path: path.into(),
            offset: 1,
            limit: None,
        }
    }

    fn call_edit(path: &Path, old: &str, new: &str) -> Call {
        Call::Edit {
            path: path.into(),
            old: old.into(),
            new: new.into(),
            all: false,
        }
    }

    /// **A Read says what it could not read, and what to use instead.**
    #[test]
    fn a_read_refuses_with_the_way_round() {
        let dir = tempfile::tempdir().unwrap();
        let caps = Caps::default();
        let said = |path: &Path| {
            let done = run(&call_read(path), &caps, &unasked());
            assert!(done.is_error && done.stamp.is_none());
            scrubbed(done).text
        };
        assert!(said(&dir.path().join("none")).starts_with("File does not exist"));
        assert!(said(dir.path()).contains("list it with `ls`"));
        let binary = dir.path().join("bin");
        std::fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
        assert!(said(&binary).contains("is not UTF-8 text"));
        let big = dir.path().join("big");
        std::fs::write(&big, "x".repeat(64)).unwrap();
        let small = Caps {
            max_file: 10,
            ..Caps::default()
        };
        let done = scrubbed(run(&call_read(&big), &small, &unasked()));
        assert!(
            done.is_error && done.text.contains("`sed -n`"),
            "{}",
            done.text
        );
    }

    /// **A Read shows its lines and stamps what it showed**; the text leaves
    /// scrubbed — a credential in the file never reaches the model.
    #[test]
    fn a_read_shows_lines_stamps_them_and_leaves_scrubbed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one\ntwo\nkey=ghp_PLANTEDTOKENVALUE0123456789\n").unwrap();
        let done = run(
            &Call::Read {
                path: path.clone(),
                offset: 2,
                limit: Some(2),
            },
            &Caps::default(),
            &unasked(),
        );
        let stamp = done.stamp.clone().unwrap();
        assert_eq!((stamp.total, stamp.seen.covers(3)), (3, false));
        assert_eq!(stamp.path, path, "by the name it was opened by");
        assert!(stamp.shown);
        let out = scrubbed(done);
        assert!(out.text.starts_with("2\ttwo\n3\tkey="), "{}", out.text);
        assert!(!out.text.contains("PLANTEDTOKEN"), "{}", out.text);
        assert!(!out.redacted.is_empty());
    }

    /// **An Edit writes in place** — a link stays a link and the file its
    /// mode — keeps a Windows file's `\r\n`, and leaves the file untouched
    /// when refused.
    #[cfg(unix)]
    #[test]
    fn an_edit_writes_in_place_and_keeps_the_file_as_it_was() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        std::fs::write(&target, "a\r\nb\r\n").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        // Through the link as it runs: at the landing judged (`Job::lands`).
        let job = Job {
            call: call_edit(&link, "b", "c\nd"),
            lands: Some(target.clone()),
            context: unasked(),
            caps: Caps::default(),
        };
        let done = scrubbed(job.run(plain()));
        assert!(
            done.text.starts_with(&format!(
                "Updated {}: lines 2–3 (were 2).",
                target.display()
            )),
            "{}",
            done.text
        );
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"a\r\nc\r\nd\r\n");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // By the name it was opened by, so a later call on it finds it.
        assert_eq!(done.stamp.unwrap().path, job.file());
        // Never through a link at its last step: one there was not judged
        // (review stop 16, E 5).
        let through = scrubbed(run(
            &call_edit(&link, "c", "x"),
            &Caps::default(),
            &unasked(),
        ));
        assert!(
            through.is_error && through.text.contains("is a link now"),
            "{}",
            through.text
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"a\r\nc\r\nd\r\n");

        let refused = run(
            &call_edit(&target, "zzz", "y"),
            &Caps::default(),
            &unasked(),
        );
        assert!(refused.is_error && refused.stamp.is_none());
        assert_eq!(std::fs::read(&target).unwrap(), b"a\r\nc\r\nd\r\n");
    }

    /// **An Edit carries forward only a stamp the file still matches** — a
    /// whole read before it stays whole; one the file changed since counts
    /// for nothing: refused under read-first, and carrying nothing without.
    #[test]
    fn an_edit_carries_only_a_current_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "1\n2\n3\n").unwrap();
        let caps = Caps::default();
        let read = run(&call_read(&path), &caps, &unasked()).stamp.unwrap();
        assert!(read.seen.covers(3));
        let any = holding(&[&read], ReadFirst::Any);
        let edited = run(&call_edit(&path, "2", "two"), &caps, &any);
        assert!(
            edited.stamp.unwrap().seen.covers(3),
            "the whole file still seen"
        );

        std::fs::write(&path, "1\n2\n3\nchanged since\n").unwrap();
        let stale = scrubbed(run(&call_edit(&path, "3", "three"), &caps, &any));
        assert!(
            stale.is_error && stale.text.contains("has changed since you read it"),
            "{}",
            stale.text
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"1\n2\n3\nchanged since\n");
        let off = holding(&[&read], ReadFirst::Off);
        let blind = run(&call_edit(&path, "3", "three"), &caps, &off);
        // Only the line the edit wrote: what the stale read saw (1–3) is gone.
        let seen = blind.stamp.unwrap().seen;
        assert_eq!(
            seen,
            crate::files::Lines::span(3, 3),
            "a stale stamp carried"
        );
    }

    /// **A change asks what `[files] read_first` asks** (F3) — under `any`,
    /// an Edit or a Write over a file nothing in the context read is
    /// refused, said, the file untouched; a part read passes; `whole` asks
    /// every line. A new file, and one that is not text, ask no read.
    #[test]
    fn a_change_asks_what_read_first_asks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "1\n2\n3\n").unwrap();
        let caps = Caps::default();
        let unread = Context::default();
        let said = |call: &Call, context: &Context| scrubbed(run(call, &caps, context));
        let edit = call_edit(&path, "2", "two");
        let write = Call::Write {
            path: path.clone(),
            content: "new\n".into(),
        };
        for call in [&edit, &write] {
            let done = said(call, &unread);
            assert!(
                done.is_error && done.text.contains("has not been read"),
                "{}",
                done.text
            );
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"1\n2\n3\n", "untouched");
        let fresh = Call::Write {
            path: dir.path().join("fresh.txt"),
            content: "x\n".into(),
        };
        assert!(!said(&fresh, &unread).is_error, "a new file asks no read");
        let binary = dir.path().join("bin");
        std::fs::write(&binary, [0xff, 0xfe]).unwrap();
        let over = Call::Write {
            path: binary,
            content: "x\n".into(),
        };
        assert!(!said(&over, &unread).is_error, "none could show it");

        let part = Call::Read {
            path: path.clone(),
            offset: 1,
            limit: Some(1),
        };
        let part = run(&part, &caps, &unread).stamp.unwrap();
        let whole = holding(&[&part], ReadFirst::Whole);
        let done = said(&edit, &whole);
        assert!(
            done.is_error && done.text.contains("only in part"),
            "{}",
            done.text
        );
        assert!(
            !said(&edit, &holding(&[&part], ReadFirst::Any)).is_error,
            "a part passes"
        );

        // Before anything is asked: an Edit with nothing read, alone.
        let unread_edit = unread.unread(&edit).expect("refused at once");
        assert!(unread_edit.contains("has not been read"), "{unread_edit}");
        assert_eq!(unread.unread(&write), None, "a Write is the run's");
        assert_eq!(holding(&[&part], ReadFirst::Any).unread(&edit), None);
        assert_eq!(unasked().unread(&edit), None, "off");
    }

    /// **An unchanged re-read points back** — lines it was shown, the file
    /// as it was, are not sent again; lines it was not shown, a file changed
    /// since, or lines it only wrote (an Edit's) are read whole.
    #[test]
    fn an_unchanged_re_read_points_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "1\n2\n3\n").unwrap();
        let caps = Caps::default();
        let read = |offset, limit, context: &Context| {
            let call = Call::Read {
                path: path.clone(),
                offset,
                limit,
            };
            scrubbed(run(&call, &caps, context))
        };
        let first = read(1, None, &unasked()).stamp.unwrap();
        let again = read(1, None, &holding(&[&first], ReadFirst::Any));
        assert!(
            again.text.contains("is unchanged since you read lines 1–3"),
            "{}",
            again.text
        );
        assert_eq!(again.stamp.as_ref().map(|s| s.shown), Some(true));
        let line = read(2, Some(1), &holding(&[&first], ReadFirst::Any));
        assert!(line.text.contains("you read line 2 of it"), "{}", line.text);

        let top = read(1, Some(1), &unasked()).stamp.unwrap();
        let wider = read(1, None, &holding(&[&top], ReadFirst::Any));
        assert!(
            wider.text.starts_with("1\t1\n2\t2"),
            "not all shown: {}",
            wider.text
        );
        let mut wrote = first.clone();
        wrote.shown = false;
        let after = read(1, None, &holding(&[&wrote], ReadFirst::Any));
        assert!(
            after.text.starts_with("1\t1"),
            "only written: {}",
            after.text
        );
        // Past the end, holding the whole file: the window is empty, and
        // says where the file ends — not "unchanged since lines 9–8"
        // (review stop 16, B 2).
        let past = read(9, None, &holding(&[&first], ReadFirst::Any));
        assert!(!past.text.contains("unchanged"), "{}", past.text);
        assert!(past.text.contains("past"), "{}", past.text);
        std::fs::write(&path, "1\n2\nthree\n").unwrap();
        let changed = read(1, None, &holding(&[&first], ReadFirst::Any));
        assert!(
            changed.text.contains("3\tthree"),
            "changed: {}",
            changed.text
        );
    }

    /// **Each tool says what it could not open, in its own words, through
    /// one door** (`open`): a directory, a file too big, one it may not
    /// write. An Edit of a directory was told the raw "Is a directory (os
    /// error 21)", an Edit of a file too big Read's `sed -n`, and Write's
    /// directory arm was held by nothing (review stop 16, D 2, mutated).
    #[cfg(unix)]
    #[test]
    fn each_tool_says_what_it_could_not_open_in_its_own_words() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (big, locked) = (dir.path().join("big"), dir.path().join("locked"));
        std::fs::write(&big, "x\n".repeat(32)).unwrap();
        std::fs::write(&locked, "a\n").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o444)).unwrap();
        let write = |path: &Path| Call::Write {
            path: path.to_path_buf(),
            content: "z\n".into(),
        };
        let small = Caps {
            max_file: 10,
            ..Caps::default()
        };
        let said = |call: Call, caps: &Caps| {
            let done = scrubbed(run(&call, caps, &unasked()));
            assert!(done.is_error, "{}", done.text);
            done.text
        };
        let d = dir.path();
        let cases = [
            (
                said(call_read(d), &small),
                "is a directory — list it with `ls`",
            ),
            (
                said(call_edit(d, "a", "b"), &small),
                "is a directory — list it with `ls`",
            ),
            (said(write(d), &small), "is a directory — name a file in it"),
            (said(call_read(&big), &small), "`sed -n`"),
            (said(call_edit(&big, "x", "y"), &small), "`sed -i`"),
            (said(write(&big), &small), "`cat > file`"),
            (
                said(call_edit(&locked, "a", "b"), &Caps::default()),
                "Cannot change",
            ),
            (said(write(&locked), &Caps::default()), "Cannot write"),
        ];
        for (text, want) in cases {
            assert!(text.contains(want), "{want:?} in {text:?}");
        }
        assert_eq!(
            std::fs::read_to_string(&locked).unwrap(),
            "a\n",
            "unchanged"
        );
    }

    /// **A Write through a link to nothing makes nothing** — the open does
    /// not follow a link at its last step (E 5), and a new file is made
    /// `O_EXCL`, which a link refuses, so the write cannot land on the file
    /// the link names, unjudged (review stop 16, B 6). Judged, a link to
    /// nothing lands on the file it names (`lands`), and the run writes
    /// that.
    #[cfg(unix)]
    #[test]
    fn a_write_through_a_link_to_nothing_makes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (link, target) = (dir.path().join("link"), dir.path().join("target"));
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let call = Call::Write {
            path: link.clone(),
            content: "x".into(),
        };
        let done = scrubbed(run(&call, &Caps::default(), &unasked()));
        assert!(
            done.is_error && done.text.contains("is a link now"),
            "{}",
            done.text
        );
        assert!(!target.exists(), "nothing made where it points");
    }

    /// **A `replace_all` costs in proportion to its matches** — four times
    /// the matches, about four times the time, on B's own shape: every
    /// other line, the file read in 2,000-line windows (as a model must
    /// read a long one), so every line is seen. Each match's line was
    /// counted from the start, and its place re-sorted every range seen, so
    /// 4,000 took 15.6 s (review stop 16, B 3, measured). One window alone
    /// hid the second: the ranges stayed few, and the cost the same at
    /// either size (mutant c6 survived it).
    #[test]
    fn a_replace_all_costs_in_proportion_to_its_matches() {
        let dir = tempfile::tempdir().unwrap();
        let cost = |matches: usize| {
            let path = dir.path().join(format!("f{matches}"));
            let body = "x\na\n".repeat(matches);
            // Seven, as its sibling below: the least of three failed 15 of
            // 33 runs beside two others (review stop 17, C 10).
            (0..7)
                .map(|_| {
                    std::fs::write(&path, &body).unwrap();
                    let stamps: Vec<Stamp> = (0..2 * matches)
                        .step_by(2000)
                        .map(|from| {
                            let window = Call::Read {
                                path: path.clone(),
                                offset: from + 1,
                                limit: Some(2000),
                            };
                            let read = run(&window, &Caps::default(), &unasked());
                            read.stamp.expect("premise: read")
                        })
                        .collect();
                    let context = holding(&stamps.iter().collect::<Vec<_>>(), ReadFirst::Whole);
                    let all = Call::Edit {
                        path: path.clone(),
                        old: "x".into(),
                        new: "y".into(),
                        all: true,
                    };
                    let started = std::time::Instant::now();
                    let done = run(&all, &Caps::default(), &context);
                    let took = started.elapsed();
                    assert!(!done.is_error, "premise: it ran");
                    took
                })
                .min()
                .unwrap()
        };
        let (small, large) = (cost(5_000), cost(20_000));
        let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
        eprintln!("5,000: {small:?}, 20,000: {large:?}, ratio {ratio:.1}");
        assert!(
            ratio < 8.0,
            "4x the matches cost {ratio:.1}x: {small:?} → {large:?}"
        );
    }

    /// **What the next call joins after a scattered `replace_all` costs in
    /// proportion too** — the file read only at its top, each replaced line
    /// its own range in the Edit's stamp (they do not touch), and every later
    /// call joins them (`Context::seen`). `add` once a range made that
    /// quadratic: 20,000 ranges, half a second (review stop 16, D 6,
    /// measured). The least of seven runs: alone the ratio is 4.0, but the
    /// least of three, microseconds each, passed 8 twice beside a hundred
    /// tests running at once (E's mutant run).
    #[test]
    fn joining_a_scattered_stamp_costs_in_proportion_to_its_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let cost = |matches: usize| {
            let path = dir.path().join(format!("g{matches}"));
            std::fs::write(&path, "x\na\n".repeat(matches)).unwrap();
            let top = Call::Read {
                path: path.clone(),
                offset: 1,
                limit: Some(10),
            };
            let read = run(&top, &Caps::default(), &unasked()).stamp.unwrap();
            let all = Call::Edit {
                path: path.clone(),
                old: "x".into(),
                new: "y".into(),
                all: true,
            };
            let edited = run(&all, &Caps::default(), &holding(&[&read], ReadFirst::Any));
            let edited = edited.stamp.expect("premise: it ran");
            assert!(
                edited.seen.holds(13, 13) && !edited.seen.holds(12, 12),
                "premise: scattered"
            );
            let context = holding(&[&read, &edited], ReadFirst::Any);
            let bytes = std::fs::read(&path).unwrap();
            (0..7)
                .map(|_| {
                    let started = std::time::Instant::now();
                    let seen = context.seen(&path, &bytes, 2 * matches).unwrap();
                    let took = started.elapsed();
                    assert!(seen.holds(1, 10), "premise: joined");
                    took
                })
                .min()
                .unwrap()
        };
        let (small, large) = (cost(5_000), cost(20_000));
        let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
        eprintln!("5,000: {small:?}, 20,000: {large:?}, ratio {ratio:.1}");
        assert!(
            ratio < 8.0,
            "4x the ranges cost {ratio:.1}x: {small:?} → {large:?}"
        );
    }

    /// **A pipe or a device is refused, at once** — by each tool, on a FIFO
    /// no one writes (a blocking open waited for a writer forever), and on a
    /// device: `/dev/zero`'s size of 0 never met the cap, and it read until
    /// memory ran out. The descriptor's own kind, asked without blocking
    /// (review stop 16, B 1, measured). `/dev/null` for the writers: a
    /// sandbox may refuse `/dev/zero` to them before its kind is asked.
    #[cfg(unix)]
    #[test]
    fn a_pipe_or_a_device_is_refused_at_once() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo");
        let c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(
            unsafe { libc::mkfifo(c.as_ptr(), 0o600) },
            0,
            "premise: a FIFO"
        );
        let writers = |path: &Path| {
            let write = Call::Write {
                path: path.to_path_buf(),
                content: "x".into(),
            };
            [call_edit(path, "a", "b"), write]
        };
        let (zero, null) = (Path::new("/dev/zero"), Path::new("/dev/null"));
        let mut calls = vec![call_read(&fifo), call_read(zero)];
        calls.extend(writers(&fifo));
        calls.extend(writers(null));
        for call in calls {
            let subject = call.subject();
            // Its own thread, so a call that waits fails the test rather than
            // hanging the suite.
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(scrubbed(run(&call, &Caps::default(), &unasked())));
            });
            let done = (rx.recv_timeout(std::time::Duration::from_secs(10)))
                .unwrap_or_else(|_| panic!("{subject}: answered, not waiting"));
            assert!(
                done.is_error && done.text.contains("not a regular file"),
                "{subject}: {}",
                done.text
            );
        }
    }

    /// **A job through a link opens the file it was judged on** — the
    /// landing, by name, though the link was pointed elsewhere since — and
    /// says so, to the model and in its subject.
    #[test]
    fn a_job_through_a_link_opens_its_judged_landing() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let (judged, moved) = (d.join("judged.txt"), d.join("moved.txt"));
        std::fs::write(&judged, "old\n").unwrap();
        std::fs::write(&moved, "old\n").unwrap();
        let link = d.join("link");
        std::os::unix::fs::symlink(&moved, &link).unwrap();
        let job = Job {
            call: Call::Edit {
                path: link.clone(),
                old: "old".into(),
                new: "new".into(),
                all: false,
            },
            lands: Some(judged.clone()),
            context: unasked(),
            caps: Caps::default(),
        };
        assert_eq!(job.file(), judged, "the landing, not the link");
        let done = scrubbed(job.run(plain()));
        assert!(!done.is_error, "{}", done.text);
        assert_eq!(std::fs::read_to_string(&judged).unwrap(), "new\n");
        assert_eq!(
            std::fs::read_to_string(&moved).unwrap(),
            "old\n",
            "not moved"
        );
        assert!(
            done.text.ends_with(&format!(
                "[through a link: {} is {}]",
                link.display(),
                judged.display()
            )),
            "{}",
            done.text
        );
        assert_eq!(
            job.subject(),
            format!("Edit {} → {}", link.display(), judged.display())
        );
    }

    /// **A Write makes a file, or replaces one in its own line endings**,
    /// says how many lines, and makes no directory.
    #[test]
    fn a_write_makes_or_replaces_and_makes_no_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        let write = |content: &str| Call::Write {
            path: path.clone(),
            content: content.into(),
        };
        let made = scrubbed(run(&write("a\nb\n"), &Caps::default(), &unasked()));
        assert_eq!(made.text, format!("Created {} (2 lines).", path.display()));
        assert!(made.stamp.unwrap().seen.covers(2));

        std::fs::write(&path, "x\r\ny\r\n").unwrap();
        let over = scrubbed(run(&write("p\nq\nr\n"), &Caps::default(), &unasked()));
        assert_eq!(
            over.text,
            format!("Wrote {} (3 lines, was 2).", path.display())
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"p\r\nq\r\nr\r\n");
        // Shorter over longer: nothing of the old file is left at its end.
        let one = scrubbed(run(&write("z\n"), &Caps::default(), &unasked()));
        assert_eq!(std::fs::read(&path).unwrap(), b"z\r\n");
        assert_eq!(
            one.text,
            format!("Wrote {} (1 line, was 3).", path.display())
        );
        let solo = dir.path().join("solo.txt");
        let call = Call::Write {
            path: solo.clone(),
            content: "Hello".into(),
        };
        let made = scrubbed(run(&call, &Caps::default(), &unasked()));
        assert_eq!(made.text, format!("Created {} (1 line).", solo.display()));

        let nested = Call::Write {
            path: dir.path().join("no/such/dir.txt"),
            content: "x".into(),
        };
        let refused = scrubbed(run(&nested, &Caps::default(), &unasked()));
        assert!(
            refused.is_error && refused.text.contains("mkdir -p"),
            "{}",
            refused.text
        );
        assert!(!dir.path().join("no").exists());
    }

    /// **A Read counts what it took from the lines it shows, and no more**
    /// — a token on line 5 is not news to a Read of line 1, and is to one
    /// of line 5; the numbers on either side of a key block stay the
    /// file's. A placeholder the file itself holds, written as ours would
    /// be, is the file's text, and cancels nothing it shows: only the
    /// lines shown are set beside the lines read.
    #[test]
    fn a_read_counts_only_what_it_took_from_its_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.env");
        let token = "key=ghp_PLANTEDTOKENVALUE0123456789";
        let (secrets, aliases) = (crate::SecretsConfig::default(), crate::Aliases::default());
        let as_ours = crate::secrets::redact(token, &secrets, &aliases).text;
        assert_ne!(as_ours, token, "premise: the token is redacted");
        std::fs::write(
            &path,
            format!(
                "a\n-----BEGIN RSA PRIVATE KEY-----\nMIIB\n-----END RSA PRIVATE KEY-----\n\
                 {token}\n{as_ours}\n"
            ),
        )
        .unwrap();
        let read = |offset, limit| {
            let call = Call::Read {
                path: path.clone(),
                offset,
                limit: Some(limit),
            };
            scrubbed(run(&call, &Caps::default(), &unasked()))
        };
        let top = read(1, 1);
        assert_eq!(
            (top.text.as_str(), top.redacted.len()),
            ("1\ta\n[lines 1–1 of 6; read on with offset 2]", 0)
        );
        let key = read(4, 2);
        assert!(
            key.text.starts_with("4\t[… continued]\n5\tkey="),
            "{}",
            key.text
        );
        assert!(!key.text.contains("PLANTED"), "{}", key.text);
        assert_eq!(
            key.redacted.len(),
            1,
            "the token, not the block above: {:?}",
            key.redacted
        );
    }

    /// **A value of every kind that runs over lines stays out of a Read** —
    /// a URL's userinfo broken by a newline, a configured literal with one,
    /// each in a file of its own, so each is what makes the Read scrub
    /// the file whole (`crosses_lines`): neither line of either shown.
    #[test]
    fn a_value_over_lines_of_any_kind_stays_out_of_a_read() {
        let dir = tempfile::tempdir().unwrap();
        let aliases = crate::Aliases::default();
        let literal = crate::SecretsConfig {
            extra: vec!["litpartone\nlitparttwo".into()],
            ..crate::SecretsConfig::default()
        };
        let plain = crate::SecretsConfig::default();
        let read = |name: &str, body: &str, secrets: &crate::SecretsConfig| {
            let path = dir.path().join(name);
            std::fs::write(&path, body).unwrap();
            let scrub = Scrub {
                secrets,
                aliases: &aliases,
            };
            let done = super::run(&call_read(&path), &Caps::default(), &unasked(), scrub);
            done.scrub(scrub).text
        };
        let url = read("url.conf", "x://userpart\nnamepart@host\n", &plain);
        assert_eq!(
            url,
            "1\tx://[redacted: url credential]\n2\t[… continued]@host"
        );
        let lit = read("lit.conf", "a litpartone\nlitparttwo b\n", &literal);
        assert_eq!(lit, "1\ta [redacted: configured value]\n2\t[… continued] b");
    }

    /// **A Read of a big file costs what it shows** — where nothing can run
    /// over a line, only the lines shown are scrubbed, not the whole file:
    /// counted, the bytes handed to the scrubber
    /// (`secrets::scrubbed_by_lines`), so it holds on any machine. Timed, it
    /// did not — an absolute bound and then a ratio, each set on this Mac,
    /// failed on Steven's Linux (11-05, 11-07), where the Read's own
    /// whole-file work weighs more against the scrub than it does here.
    #[test]
    fn a_read_of_a_big_file_costs_what_it_shows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.rs");
        let line = "    let token_value = compute(alpha, beta) + 42; // a note\n";
        let text = line.repeat(10_000);
        std::fs::write(&path, &text).unwrap();
        let call = Call::Read {
            path,
            offset: 1,
            limit: Some(10),
        };
        let before = crate::secrets::scrubbed_by_lines();
        let done = scrubbed(run(&call, &Caps::default(), &unasked()));
        let handed = crate::secrets::scrubbed_by_lines() - before;
        assert!(!done.is_error, "{}", done.text);
        assert!(done.text.contains("\n10\t"), "{}", done.text);
        // Above zero: the count is live, so its smallness is the Read's.
        assert!(
            0 < handed && handed <= 10 * line.len(),
            "{handed} bytes scrubbed for 10 lines of a {}-byte file",
            text.len()
        );
    }

    /// **A link planted after the judgment is not written through** (review
    /// stop 16, E 5, measured: a Write judged with nothing at `b` wrote the
    /// file a link put there since named). Refused, said; the file it names
    /// untouched.
    #[test]
    fn a_link_planted_after_the_judgment_is_not_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let (b, theirs) = (dir.path().join("b"), dir.path().join("theirs"));
        std::fs::write(&theirs, [0u8, 1, 2]).unwrap();
        let job = Job {
            call: Call::Write {
                path: b.clone(),
                content: "pwned\n".into(),
            },
            lands: None,
            context: unasked(),
            caps: Caps::default(),
        };
        std::os::unix::fs::symlink(&theirs, &b).unwrap();
        let done = scrubbed(job.run(plain()));
        assert!(
            done.is_error && done.text.contains("is a link now"),
            "{}",
            done.text
        );
        assert_eq!(std::fs::read(&theirs).unwrap(), [0u8, 1, 2]);
    }

    /// A disk that fills after `room` bytes — and when `once`, has room again
    /// after its first refusal, for the put-back.
    struct Filling {
        body: std::io::Cursor<Vec<u8>>,
        room: usize,
        once: bool,
    }

    impl std::io::Write for Filling {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.room == 0 {
                if std::mem::take(&mut self.once) {
                    self.room = usize::MAX;
                }
                return Err(std::io::Error::other("No space left on device"));
            }
            let n = buf.len().min(self.room);
            self.room -= n;
            self.body.write(&buf[..n])
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl std::io::Seek for Filling {
        fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
            self.body.seek(to)
        }
    }

    impl Body for Filling {
        fn set_len(&mut self, len: u64) -> std::io::Result<()> {
            self.body.get_mut().resize(len as usize, 0);
            Ok(())
        }
    }

    /// **A write that fails keeps the most of the file, and says what it
    /// left** (review stop 16, E 7: the file was cut to nothing first, and
    /// a full disk left it empty under "Cannot write"). A shorter text over
    /// a longer one leaves none of the old tail; a failed one is put back,
    /// or, where that fails too, said to be part-written — the old text's
    /// tail still there, never an empty file.
    #[test]
    fn a_failed_write_puts_the_file_back_or_says_it_may_be_part_written() {
        let old = b"the old text, longer\n";
        let disk = |room, once| Filling {
            body: std::io::Cursor::new(old.to_vec()),
            room,
            once,
        };
        let path = Path::new("/p/a");
        let mut room = disk(usize::MAX, false);
        rewrite(&mut room, path, b"new\n", old).unwrap();
        assert_eq!(room.body.get_ref(), b"new\n");

        let mut back = disk(2, true);
        let said = rewrite(&mut back, path, b"brand new text\n", old).unwrap_err();
        assert!(said.ends_with("its text was put back as it was"), "{said}");
        assert_eq!(back.body.get_ref(), old);

        let mut stuck = disk(2, false);
        let said = rewrite(&mut stuck, path, b"brand new text\n", old).unwrap_err();
        assert!(
            said.contains("No space left") && said.contains("part-written"),
            "{said}"
        );
        assert_eq!(
            stuck.body.get_ref().len(),
            old.len(),
            "never cut to nothing"
        );
    }

    /// **A file larger than it said is refused, not read short** (review
    /// stop 16, E 8: deleting the check passed every test, and an Edit
    /// wrote back the first `max_file + 1` bytes). A size of 0 is what
    /// procfs reports.
    #[test]
    fn a_file_larger_than_it_said_is_refused() {
        let mut big = std::io::Cursor::new(vec![b'x'; 20]);
        assert_eq!(capped(&mut big, 0, 10).unwrap(), Err(11));
        let mut fits = std::io::Cursor::new(vec![b'x'; 10]);
        assert_eq!(capped(&mut fits, 0, 10).unwrap(), Ok(vec![b'x'; 10]));
    }

    /// **A new file a held secret is written into is yours alone** (review
    /// stop 16, E 10) — owner-only; one without takes your umask, as a
    /// redirect's file does.
    #[test]
    fn a_new_file_holding_a_secret_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let mut aliases = crate::Aliases::default();
        aliases.insert("db", "planted-db-value", "t", true).unwrap();
        let secrets = crate::SecretsConfig::default();
        let scrub = Scrub {
            secrets: &secrets,
            aliases: &aliases,
        };
        let mode = |name: &str, content: &str| {
            let path = dir.path().join(name);
            let call = Call::Write {
                path: path.clone(),
                content: content.into(),
            };
            let done = super::run(&call, &Caps::default(), &unasked(), scrub);
            assert!(!done.is_error, "{}", done.text);
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777
        };
        assert_eq!(mode("secret.env", "KEY=planted-db-value\n"), 0o600);
        let plain = mode("plain.env", "KEY=other\n");
        assert_ne!(plain, 0o600, "premise: the umask gives a plain file more");
    }

    proptest::proptest! {
        /// **A held value never reaches a Read's text** (review stop 16,
        /// E 1) — whole or any line of it, wherever it stands: across
        /// lines, at any column, inside the cut, at the window's edges.
        /// The value is the only text with capitals, and nothing a Read
        /// adds has one (the premise is asserted), so a capital in the
        /// result is a piece of the value.
        #[test]
        fn a_held_value_never_reaches_a_read(
            before in "[a-z \n]{0,40}",
            value in "[A-Z]{1,6}(\n[A-Z]{0,6}){0,3}",
            after in "[a-z \n]{0,40}",
            line_chars in 1usize..12,
            max_bytes in 1usize..200,
            offset in 0usize..8,
            limit in proptest::option::of(1usize..6),
        ) {
            let mut aliases = crate::Aliases::default();
            let handle = aliases.insert("db", &value, "t", true).unwrap();
            proptest::prop_assert!(!handle.chars().any(|c| c.is_ascii_uppercase()));
            let secrets = crate::SecretsConfig::default();
            let scrub = Scrub { secrets: &secrets, aliases: &aliases };
            let caps = Caps { line_chars, max_bytes, ..Caps::default() };
            let text = Text::decode(format!("{before}{value}{after}").as_bytes()).unwrap();
            let (window, _) = shown(&text, offset, limit, &caps, scrub);
            proptest::prop_assert_eq!(window.total, text.lines(), "the numbers are the file's");
            let sent = Done::did(window.report(), Stamp::written(PathBuf::new(), b"", 0), Vec::new())
                .scrub(scrub)
                .text;
            proptest::prop_assert!(
                !sent.chars().any(|c| c.is_ascii_uppercase()),
                "{:?} in {:?} reached the model as {:?}",
                value,
                text.body(),
                sent
            );
        }
    }
}
