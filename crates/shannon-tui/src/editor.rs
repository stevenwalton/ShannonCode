//! The `$EDITOR` escape hatch (criterion 9).
//!
//! `Ctrl-X Ctrl-E` opens the current prompt in the user's `$EDITOR` (a
//! tempfile whose extension is the content's [`Filetype`]); on save-and-quit the edited text replaces the prompt buffer and
//! is **not** auto-submitted (execute-on-save is a future opt-in). `:Edit`
//! opens the transcript read-only the same way (changes discarded). The TUI is
//! suspended/resumed around the child via [`with_terminal_suspended`], which
//! restores the terminal even if the spawn errors or the editor crashes.
//!
//! The orchestration (tempfile, read-back, cleanup) lives here; the pure
//! `$EDITOR`-string parsing is factored out so it's unit-testable without
//! spawning a process.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::terminal::{ShannonTerminal, with_terminal_suspended};

/// **What is being edited**, which decides the tempfile's extension — and so
/// the editor's filetype, highlighting and comment syntax.
///
/// Every session used to get `.md`, which is right for a prompt and wrong for
/// the rest: `:config edit` opened TOML as markdown, so every `#` comment
/// rendered as a heading (Steven, 2026-09-24). The type belongs to the
/// CONTENT, which only the caller knows, so the caller says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filetype {
    /// Prose to the model: a prompt, a briefing, a squash seed, a transcript.
    Markdown,
    /// A shell command the model proposed, edited before it runs.
    Shell,
    /// The config file, or its defaults reference.
    Toml,
    /// Anything else: a listing with no syntax of its own.
    Text,
    /// A file's new text, edited before Claude Code writes it, by the
    /// extension of the file it goes to ([`Filetype::of_path`]).
    Code(&'static str),
}

/// The extensions a written file's edit opens under, so the editor colours
/// it as the language it is. **A list, not the path's own text**: the path
/// is the model's, and the extension becomes part of a file name Shannon
/// creates — `x.rs/../../y` must never steer where that is.
const CODE_EXTENSIONS: &[&str] = &[
    "c", "cc", "cpp", "css", "go", "h", "hpp", "html", "java", "js", "json", "jsx", "kt", "lua",
    "md", "nix", "py", "rb", "rs", "sh", "sql", "swift", "toml", "ts", "tsx", "txt", "vim", "yaml",
    "yml", "zsh",
];

impl Filetype {
    fn extension(self) -> &'static str {
        match self {
            Filetype::Markdown => "md",
            Filetype::Shell => "sh",
            Filetype::Toml => "toml",
            Filetype::Text => "txt",
            Filetype::Code(extension) => extension,
        }
    }

    /// **The filetype of a file's text, by its path's extension** — one of
    /// a fixed list (`CODE_EXTENSIONS`), matched case-blind, else plain text.
    pub fn of_path(path: &str) -> Filetype {
        let extension = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        CODE_EXTENSIONS
            .iter()
            .find(|known| known.eq_ignore_ascii_case(extension))
            .map_or(Filetype::Text, |known| Filetype::Code(known))
    }
}

/// Open `initial` in `$EDITOR` and return the edited text (one trailing newline
/// stripped — editors add one, and the prompt shouldn't gain a blank last
/// line). The TUI is suspended around the editor and resumed after. Errors
/// (no editor, non-zero exit, I/O) propagate *after* the terminal is restored.
///
/// Returns the file's content whether or not the user saved — right for flows
/// with their own no-op/emptiness checks (the prompt edit, `e`, `:config
/// edit`). A flow where *applying* is consequential (a squash review) wants
/// [`edit_text_saved`], where not saving cancels.
pub fn edit_text(
    terminal: &mut ShannonTerminal,
    mouse: bool,
    initial: &str,
    kind: Filetype,
) -> Result<String> {
    edit_session(terminal, mouse, initial, kind).map(|(text, _saved)| text)
}

/// Like [`edit_text`], but **`None` if the user quit without saving** (`:q!`,
/// `:q`) — the save is the approval signal, so an abandoned session cancels
/// instead of applying the seeded content. Detected by the file's mtime: an
/// editor writes on `:wq`/`:w` and doesn't on a bare quit. (`:cq` — nonzero
/// exit — still cancels harder, as an `Err`.) Used by the squash flows, where
/// the seed alone is a *draft*, not a decision.
pub fn edit_text_saved(
    terminal: &mut ShannonTerminal,
    mouse: bool,
    initial: &str,
    kind: Filetype,
) -> Result<Option<String>> {
    edit_session(terminal, mouse, initial, kind).map(|(text, saved)| saved.then_some(text))
}

/// **What opens `$EDITOR` on a text, and reads it back** — saved, quit
/// unsaved, or failed, as [`edit_text_saved`] says them. The terminal's
/// ([`Suspended`]), or a test's.
///
/// A decision's editor is a key pressed in its pane — a call's `e`, a
/// summary review's `<CR>` (`decision-panes-and-alerts.md` §4) — and what
/// it opens on, and what comes back of it, is the half of the decision no
/// test could reach while it took the terminal itself: tests drove the
/// halves either side (`apply_edit_for_test`, `land_summary_review`).
pub(crate) trait Editor {
    /// `initial`, edited as `kind`: `Some` saved, `None` quit unsaved, an
    /// error when the editor failed or exited non-zero (`:cq`).
    fn edit_saved(&mut self, initial: &str, kind: Filetype) -> Result<Option<String>>;
}

/// **The terminal's `$EDITOR`**, the TUI suspended around it.
pub(crate) struct Suspended<'a> {
    pub(crate) terminal: &'a mut ShannonTerminal,
    /// Whether the TUI captures the mouse, to give back on resuming.
    pub(crate) mouse: bool,
}

impl Editor for Suspended<'_> {
    fn edit_saved(&mut self, initial: &str, kind: Filetype) -> Result<Option<String>> {
        edit_text_saved(self.terminal, self.mouse, initial, kind)
    }
}

/// The shared editor session: tempfile out, `$EDITOR`, text back, plus whether
/// the editor actually **wrote** the file ([`session_with`]).
fn edit_session(
    terminal: &mut ShannonTerminal,
    mouse: bool,
    initial: &str,
    kind: Filetype,
) -> Result<(String, bool)> {
    session_with(initial, kind, |path| run_editor(terminal, mouse, path))
}

/// **The file half of an editor session**, around `run` on the file: the
/// tempfile out owner-only, the text back, and the file removed on every
/// path after its create. `run` is the terminal's `$EDITOR` in
/// [`edit_session`], a closure in a test — the round trip no test could
/// reach while the terminal was the only runner (review stop 8, B 7).
///
/// **Saved** is the mtime moved, or the text changed: a `:w` inside the
/// filesystem's timestamp granularity (a second on HFS+, FAT, some network
/// homes) left the mtime where it was, and an edit read as quit unsaved —
/// dropped, and the review marked "closed unsaved" (B 8). When the mtime
/// can't be read the session counts as saved: failing open preserves the
/// user's work; failing closed would silently discard an edit.
fn session_with(
    initial: &str,
    kind: Filetype,
    run: impl FnOnce(&Path) -> Result<()>,
) -> Result<(String, bool)> {
    let path = temp_path(kind);
    write_private(&path, initial)?;
    let before = mtime(&path);
    let outcome = run(&path).and_then(|()| {
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let saved = match (before, mtime(&path)) {
            (Some(b), Some(a)) => a != b || text != initial,
            _ => true, // unreadable mtime: assume saved, never discard silently
        };
        Ok((text, saved))
    });
    let _ = std::fs::remove_file(&path); // best-effort cleanup; temp dir anyway
    let (text, saved) = outcome?;
    Ok((text.strip_suffix('\n').unwrap_or(&text).to_string(), saved))
}

/// The file's modification time, if readable.
fn mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Open `content` in `$EDITOR` **read-only** (`:Edit` the transcript, `:config
/// dump`): the tempfile is made read-only at the filesystem level, so any
/// editor signals it (vim `[readonly]`) and won't prompt to save on quit — the
/// honest signal that this is a view, not an edit (changes would be discarded
/// anyway, since we never read the file back). Editor-agnostic: it relies on
/// the file's permission bit, not an editor-specific flag.
pub fn view_text(
    terminal: &mut ShannonTerminal,
    mouse: bool,
    content: &str,
    kind: Filetype,
) -> Result<()> {
    let path = temp_path(kind);
    write_private(&path, content)?;
    // After a successful create, cleanup must run on *every* path — even if the
    // read-only chmod fails — so a chmod error can't strand the tempfile. (Hence
    // `and_then`, not `?`, on the read-only step.)
    let result = set_readonly(&path, true).and_then(|()| run_editor(terminal, mouse, &path));
    // Restore write so the unlink succeeds on Windows (Unix unlinks regardless
    // of the file's own mode — the directory's matters); both best-effort.
    let _ = set_readonly(&path, false);
    let _ = std::fs::remove_file(&path);
    result
}

/// Create `path` for an editor session with **owner-only** permissions and write
/// `content`. `create_new` (`O_EXCL`) refuses to follow a pre-existing file or
/// symlink at our predictable `pid+nanos` temp name — defeating a symlink-swap
/// that would otherwise make us truncate or expose a victim's file — and the
/// `0600` mode keeps the prompt/transcript text unreadable to other users on a
/// shared host (a plain `fs::write` inherits the umask, typically `0644`).
/// Non-unix falls back to a fresh create (Windows ACLs differ).
///
/// The implementation now lives in [`private_file`](crate::private_file),
/// shared with the config writer; this alias keeps the call sites reading in
/// editor terms.
fn write_private(path: &Path, content: &str) -> Result<()> {
    crate::private_file::create_new(path, content)
}

/// Set (or clear) the read-only permission bit on `path`. Cross-platform via
/// `Permissions::set_readonly` (clears the Unix write bits / sets the Windows
/// read-only attribute).
fn set_readonly(path: &Path, readonly: bool) -> Result<()> {
    let mut perms = std::fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .permissions();
    perms.set_readonly(readonly);
    std::fs::set_permissions(path, perms).with_context(|| format!("chmod {}", path.display()))
}

/// Suspend the TUI, spawn `$EDITOR <path>`, wait, resume. Separated so both
/// `edit_text` and `view_text` share the suspend/spawn/resume dance — and the
/// one door for editing a REAL file in place: `:memories` and `:skills` open
/// the file under the cursor, which is the point (Steven, 10-06).
pub(crate) fn run_editor(terminal: &mut ShannonTerminal, mouse: bool, path: &Path) -> Result<()> {
    // `with_terminal_suspended` resumes the TUI on the way out (even on panic);
    // the inner `Result` is the editor spawn/exit status, propagated after.
    with_terminal_suspended(terminal, mouse, || spawn_editor(path))?
}

fn spawn_editor(path: &Path) -> Result<()> {
    let editor = editor_command();
    let (cmd, args) = split_editor(&editor);
    let status = Command::new(cmd)
        .args(args)
        .arg(path)
        .status()
        .with_context(|| format!("spawn $EDITOR ({editor})"))?;
    if !status.success() {
        bail!("$EDITOR ({editor}) exited with {status}");
    }
    Ok(())
}

/// The configured editor: `$VISUAL`, then `$EDITOR`, then `vi` — the standard
/// fallback chain (`VISUAL` wins for full-screen editors, as in git).
fn editor_command() -> String {
    std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "vi".to_string())
}

/// Split an `$EDITOR` string into `(command, args)` on whitespace, so a value
/// like `"code --wait"` runs `code` with `--wait`. (Whitespace-split, so an
/// editor *path* containing spaces isn't supported — acceptable; the common
/// editors don't.) Empty → `vi`.
fn split_editor(editor: &str) -> (&str, Vec<&str>) {
    let mut parts = editor.split_whitespace();
    let cmd = parts.next().unwrap_or("vi");
    (cmd, parts.collect())
}

/// A unique temp path for one editor session:
/// `$TMPDIR/shannon-edit-<pid>-<nanos>-<n>.<ext>`; the extension comes from
/// `kind`, and is what gives the editor its filetype.
///
/// **`n`, a count in this process**, because pid and time are not unique
/// within one: a TUI edits one file at a time, but its tests run in parallel
/// threads, and macOS's clock ticks in microseconds — two drew one path, and
/// the second's owner-only create (`O_EXCL`) failed about one full run in
/// thirty (review stop 15, C L-j).
fn temp_path(kind: Filetype) -> PathBuf {
    static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "shannon-edit-{}-{}-{n}.{}",
        std::process::id(),
        nanos,
        kind.extension()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_editor_parses_command_and_args() {
        assert_eq!(split_editor("nvim"), ("nvim", vec![]));
        assert_eq!(split_editor("code --wait"), ("code", vec!["--wait"]));
        assert_eq!(split_editor("  emacs  -nw "), ("emacs", vec!["-nw"]));
        assert_eq!(split_editor(""), ("vi", vec![]));
    }

    #[test]
    fn temp_path_carries_the_content_type() {
        for (kind, ext) in [
            (Filetype::Markdown, "md"),
            (Filetype::Shell, "sh"),
            (Filetype::Toml, "toml"),
            (Filetype::Text, "txt"),
        ] {
            let p = temp_path(kind);
            assert!(p.starts_with(std::env::temp_dir()));
            assert_eq!(
                p.extension().and_then(|e| e.to_str()),
                Some(ext),
                "{kind:?}"
            );
        }
    }

    /// **Two paths drawn at once are two paths** — however fast the clock
    /// ticks (review stop 15, C L-j: parallel tests drew one).
    #[test]
    fn temp_paths_drawn_together_differ() {
        let threads: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    (0..2000)
                        .map(|_| temp_path(Filetype::Text))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let paths: std::collections::HashSet<PathBuf> = threads
            .into_iter()
            .flat_map(|t| t.join().expect("drawn"))
            .collect();
        assert_eq!(paths.len(), 8 * 2000);
    }

    /// **A written file's edit opens as its language, by a fixed list** — the
    /// path is the model's, and a name built from its text could leave the
    /// temp directory: an extension the list does not hold opens as text.
    #[test]
    fn a_written_files_extension_is_taken_from_a_list() {
        assert_eq!(Filetype::of_path("src/main.RS"), Filetype::Code("rs"));
        assert_eq!(Filetype::of_path("Makefile"), Filetype::Text);
        for hostile in [
            "a.rs/../../../etc/x",
            "x.rs\0.sh",
            "x./../y",
            "x.toolongtobeanything",
        ] {
            let kind = Filetype::of_path(hostile);
            let path = temp_path(kind);
            assert_eq!(
                path.parent(),
                Some(std::env::temp_dir().as_path()),
                "{hostile}"
            );
            assert!(
                matches!(kind, Filetype::Text | Filetype::Code(_)),
                "{hostile}"
            );
        }
        assert_eq!(Filetype::of_path("x./../y.py"), Filetype::Code("py"));
    }

    #[cfg(unix)]
    #[test]
    fn write_private_creates_an_owner_only_file() {
        // The prompt/transcript draft must not be world-readable on a shared host.
        use std::os::unix::fs::PermissionsExt as _;
        let path = temp_path(Filetype::Markdown);
        write_private(&path, "secret draft").expect("create private");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "tempfile must be owner-only");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "secret draft");
        std::fs::remove_file(&path).ok();
    }

    /// **An editor session's file half, round trip** (review stop 8, B 7,
    /// B 8): saved text comes back less the editor's newline; quit unsaved
    /// comes back as it went, not saved; a write that leaves the mtime where
    /// it was — inside the filesystem's granularity — is saved; and the file
    /// is gone after, whether the editor succeeded or failed.
    #[test]
    fn an_editor_session_reads_back_and_leaves_no_file() {
        let seen = std::cell::RefCell::new(None::<PathBuf>);
        let saved = session_with("draft", Filetype::Markdown, |path| {
            *seen.borrow_mut() = Some(path.to_path_buf());
            std::fs::write(path, "edited\n").map_err(Into::into)
        });
        assert_eq!(saved.unwrap(), ("edited".to_string(), true));
        let path = seen.borrow_mut().take().expect("it ran");
        assert!(!path.exists(), "removed after: {}", path.display());

        let unsaved = session_with("draft", Filetype::Markdown, |_| Ok(()));
        assert_eq!(unsaved.unwrap(), ("draft".to_string(), false));

        let same_tick = session_with("draft", Filetype::Markdown, |path| {
            let before = std::fs::metadata(path)?.modified()?;
            std::fs::write(path, "edited")?;
            std::fs::File::options()
                .write(true)
                .open(path)?
                .set_modified(before)?;
            Ok(())
        });
        assert_eq!(same_tick.unwrap(), ("edited".to_string(), true));

        let failed = session_with("draft", Filetype::Markdown, |path| {
            *seen.borrow_mut() = Some(path.to_path_buf());
            anyhow::bail!("exited with 1")
        });
        assert!(failed.is_err());
        let path = seen.borrow_mut().take().expect("it ran");
        assert!(
            !path.exists(),
            "removed after a failure: {}",
            path.display()
        );
    }

    #[test]
    fn set_readonly_toggles_the_permission_bit() {
        // The mechanism behind `:Edit`/`:config dump` opening read-only.
        let path = temp_path(Filetype::Text);
        std::fs::write(&path, "x").expect("write temp");
        set_readonly(&path, true).expect("set ro");
        assert!(std::fs::metadata(&path).unwrap().permissions().readonly());
        set_readonly(&path, false).expect("clear ro");
        assert!(!std::fs::metadata(&path).unwrap().permissions().readonly());
        std::fs::remove_file(&path).ok();
    }
}
