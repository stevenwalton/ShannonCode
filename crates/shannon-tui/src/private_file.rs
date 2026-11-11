//! Writing a file that only its owner may read.
//!
//! Three surfaces already hold this rule independently — the episode store
//! (`0600` logs in a `0700` dir), the log dir, and the `$EDITOR` tempfile — and
//! the config file made a fourth. A plain [`std::fs::write`] inherits the umask
//! (typically `0644`), so "I saved your file" quietly means "…and your
//! neighbours on this host can read it." Every file Shannon creates on the user's
//! behalf carries conversation text, credentials-adjacent config, or both, so
//! the default is wrong for all of them (`[[feedback_persisted_data_perms]]`).
//!
//! The three functions here differ in what they do to a file already there,
//! and the difference is the security property in each case:
//!
//! - [`create_new`] refuses a path that already exists (`O_EXCL`). For a
//!   tempfile at a *predictable* name, that refusal is the point: it defeats a
//!   symlink swapped in ahead of us, which would otherwise make us truncate or
//!   expose someone else's file.
//! - [`overwrite`] replaces an existing file, which is what saving a config the
//!   user just edited means. It keeps the mode only for the file it *creates*;
//!   a pre-existing file's mode belongs to the user, and silently tightening it
//!   would be us editing their filesystem behind their back.
//! - [`replace`] swaps in a new owner-only file whatever was there — for a
//!   credential, where the old file's mode must NOT survive.
//!
//! Non-unix falls back to a plain create in both cases (Windows ACLs are a
//! different model, and we do not yet claim to support it).

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;

use anyhow::{Context, Result};

/// `0600` on creation, on unix. Shared by both entry points so the mode lives
/// in one place rather than at each call site.
fn owner_only(opts: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let _ = opts;
}

/// Create `path` owner-only and write `content`, **failing if it already
/// exists**. See the module docs for why the refusal is the security property
/// rather than an inconvenience.
pub fn create_new(path: &Path, content: &str) -> Result<()> {
    create_new_writing(path, |f| f.write_all(content.as_bytes()))
}

/// [`create_new`] with the write given — the seam its failure is tested
/// through, since no portable file refuses a write once opened.
fn create_new_writing(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<()> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    owner_only(&mut opts);
    let mut f = opts
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    // Ours — `O_EXCL` made it — so a write that fails takes it back: a
    // partial copy of what it was given (a draft, a key) must not outlive
    // the call that reported failing (review stop 8, B 7). Best-effort.
    write(&mut f).map_err(|e| {
        let _ = std::fs::remove_file(path);
        anyhow::Error::new(e).context(format!("write {}", path.display()))
    })
}

/// `parent` and any missing ancestors, created `0700` — the one place the
/// directory mode lives, for every writer that creates one ([`overwrite`],
/// [`replace`], and `claude_sessions`' append, which made its own with the
/// umask's mode while its doc said `0700`: review stop 3, B).
pub(crate) fn owner_only_dir(parent: &Path) -> Result<()> {
    let mut dir = std::fs::DirBuilder::new();
    dir.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        dir.mode(0o700);
    }
    dir.create(parent)
        .with_context(|| format!("create {}", parent.display()))
}

/// Write `content` to `path`, creating parent directories (`0700`) and the file
/// (`0600`) if they do not exist, and truncating if they do.
pub fn overwrite(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        owner_only_dir(parent)?;
    }
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    owner_only(&mut opts);
    let mut f = opts
        .open(path)
        .with_context(|| format!("write {}", path.display()))?;
    f.write_all(content.as_bytes())
        .with_context(|| format!("write {}", path.display()))
}

/// **Replace `path` with `content`, owner-only whatever was there** — for a
/// credential, where [`overwrite`]'s deference to an existing file's mode is
/// wrong: a key file left `0644` must not stay readable because it existed.
///
/// Written to a fresh `0600` file beside it (`O_EXCL`, so nothing planted at
/// the temp name is followed), then renamed over the target — which replaces
/// a symlink at `path` rather than writing through it, and never leaves a
/// half-written key behind. Parent directories are created `0700`.
pub fn replace(path: &Path, content: &str) -> Result<()> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    owner_only_dir(parent)?;
    let name = path
        .file_name()
        .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let temp = parent.join(format!(".{name}.{}.{nonce:x}.tmp", std::process::id()));
    create_new(&temp, content)?;
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        anyhow::Error::new(e).context(format!("replace {}", path.display()))
    })
}

/// **The premise every owner-only test leans on**: a file written plainly in
/// `dir` is NOT owner-only already. Under a `077` umask it would be — and a
/// directory `0700` — so every `0600`/`0700` assertion would hold of code that
/// set nothing (review stop 12, T7 16). One door, for every test of a mode:
/// this crate's writers (`private_file`), the logs, the trust and session
/// records.
#[cfg(all(test, unix))]
pub(crate) fn assert_umask_grants_more(dir: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let probe = dir.join(".umask-probe");
    std::fs::write(&probe, b"x").expect("the probe, written");
    let mode = std::fs::metadata(&probe)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777;
    std::fs::remove_file(&probe).expect("the probe, removed");
    assert_ne!(
        mode & 0o077,
        0,
        "premise: a plain file in {} is {mode:o} — owner-only under this umask, so an \
         owner-only check here would measure nothing",
        dir.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A failed create leaves no copy behind** — a write that fails part
    /// way takes back the file `O_EXCL` made, so half a secret does not
    /// outlive the error (review stop 8, B 7; untested until review stop
    /// 12, T7).
    #[test]
    fn a_failed_create_leaves_no_copy_of_the_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("draft");
        let failed = create_new_writing(&path, |f| {
            f.write_all(b"sk-sec")?;
            Err(std::io::Error::other("the disk is full"))
        });
        assert!(failed.is_err(), "the write's failure is the call's");
        assert!(!path.exists(), "and the part written went with it");
    }

    /// **A failed replace leaves no copy behind** — the temp file holds the
    /// whole credential under a name nothing protects, so a rename that fails
    /// must take it away (review 21, Reviewer C).
    #[test]
    fn a_failed_replace_leaves_no_copy_of_the_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("api_key");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("in-the-way"), "x").unwrap();
        assert!(
            replace(&target, "sk-secret").is_err(),
            "a full directory is in the way"
        );
        let left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["api_key"], "{left:?}");
    }

    #[cfg(unix)]
    #[test]
    fn replace_leaves_a_credential_owner_only_whatever_was_there() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        assert_umask_grants_more(dir.path());
        let path = dir.path().join("cfg/api_key");
        replace(&path, "first").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir.path().join("cfg")), 0o700);
        // A key file the user left readable is not left readable.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        replace(&path, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        assert_eq!(mode(&path), 0o600);
        // A symlink at the path is replaced, not written through.
        let elsewhere = dir.path().join("elsewhere");
        std::fs::write(&elsewhere, "theirs").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        replace(&path, "third").unwrap();
        assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "theirs");
        assert!(
            !std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        // No temp file left beside it.
        let left: Vec<_> = std::fs::read_dir(dir.path().join("cfg"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left.len(), 1, "{left:?}");
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn create_new_refuses_an_existing_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        create_new(&path, "first").unwrap();
        // The refusal IS the property: a symlink planted at our predictable temp
        // name must not become a file we truncate.
        let err = create_new(&path, "second").expect_err("must refuse");
        assert!(
            format!("{err:#}").contains("create"),
            "and says which step refused, so the caller can report it: {err:#}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        // A DANGLING link is the shape that tells `O_EXCL` from a check
        // first: `exists()` follows the link and says no, and the open that
        // follows creates the file it points at (review stop 12, T7 5).
        #[cfg(unix)]
        {
            let planted = dir.path().join("planted");
            let victim = dir.path().join("victim-not-yet");
            std::os::unix::fs::symlink(&victim, &planted).unwrap();
            assert!(create_new(&planted, "ours").is_err(), "a link is refused");
            assert!(!victim.exists(), "nothing was written through the link");
        }
    }

    #[test]
    fn overwrite_replaces_and_creates_the_parent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deeper/conf.toml");
        overwrite(&path, "a = 1").unwrap();
        overwrite(&path, "a = 2").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a = 2");
    }

    #[cfg(unix)]
    #[test]
    fn created_files_and_dirs_are_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        assert_umask_grants_more(dir.path());
        let nested = dir.path().join("cfg");
        let path = nested.join("conf.toml");
        overwrite(&path, "a = 1").unwrap();
        assert_eq!(mode(&path), 0o600, "config file must be owner-only");
        assert_eq!(mode(&nested), 0o700, "config dir must be owner-only");

        let fresh = dir.path().join("fresh");
        create_new(&fresh, "x").unwrap();
        assert_eq!(mode(&fresh), 0o600, "created file must be owner-only");

        // Every ancestor it makes, not only the last: the intermediate ones
        // were unmeasured (review stop 12, T7).
        let deep = dir.path().join("a/b/c/conf.toml");
        overwrite(&deep, "a = 1").unwrap();
        for made in ["a", "a/b", "a/b/c"] {
            assert_eq!(
                mode(&dir.path().join(made)),
                0o700,
                "{made} must be owner-only"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn overwrite_leaves_an_existing_files_mode_alone() {
        // The user's file, the user's mode. We protect what we create; we do not
        // silently re-permission what we find.
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theirs");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        overwrite(&path, "new").unwrap();
        assert_eq!(mode(&path), 0o640);
    }
}
