//! **The scratch directory** — the agent's own place for temporary files
//! (Steven, 10-06: "The specific directory in /tmp like /tmp/shannon/. Note
//! that this should be a configurable location, but that is the default").
//! An approved place at every level (`permission::RuleSet::with_scratch`),
//! and where Claude Code keeps its own (`CLAUDE_CODE_TMPDIR`).
//!
//! **Ownership, not secrecy.** `/tmp` is shared, so a fixed name there is
//! one another account can make first: as its own directory, to read what is
//! written there, or as a symlink, to aim a write at this user's files. A
//! random name answers only the first, by hiding, and leaves a name nobody
//! can type or write a rule for. So the default is per-user
//! (`/tmp/shannon-<uid>`, as Claude Code's is `/tmp/claude-<uid>`), and
//! whatever the name, it is used only as a real directory, this user's, shut
//! to everyone else — checked at every launch, never assumed. Inside it,
//! only this user can make a name, so nothing there was planted by another.

use std::path::{Path, PathBuf};

/// `/tmp/shannon-<uid>` — `[permissions] scratch` when it says none.
pub fn default_path() -> PathBuf {
    PathBuf::from(format!("/tmp/shannon-{}", uid()))
}

/// **Make `path` the scratch directory**, or say why it cannot be: created
/// owner-only if missing, then held to being a directory (not a symlink to
/// one), this user's, and closed to every other account. `Ok` holds the
/// spellings a command may use for it — as configured, and resolved (`/tmp`
/// is `/private/tmp` on macOS).
pub fn prepare(path: &Path) -> Result<Vec<String>, String> {
    prepare_shut(path, Shut::Everything)
}

/// **Make `path` a place the model writes memories in unasked** — as
/// [`prepare`], but others may READ it. It lives in your home, where your
/// umask decides who reads; what an unasked write there must not meet is a
/// name another account planted — a link aiming the write at your files —
/// and that takes their WRITE access. Claude Code makes its memory
/// directories `0755`, which [`prepare`] refuses (Steven, 10-06).
pub fn prepare_owned(path: &Path) -> Result<Vec<String>, String> {
    prepare_shut(path, Shut::Writing)
}

/// What other accounts may not do in a vouched directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shut {
    /// Anything: the scratch directory, in a shared `/tmp`, holds output.
    Everything,
    /// Write: a memory location ([`prepare_owned`]).
    Writing,
}

impl Shut {
    /// The mode bits that must be clear.
    fn mask(self) -> u32 {
        match self {
            Shut::Everything => 0o077,
            Shut::Writing => 0o022,
        }
    }
}

fn prepare_shut(path: &Path, shut: Shut) -> Result<Vec<String>, String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let shown = path.display();
    if !path.is_absolute() {
        return Err(format!("{shown} is not an absolute path"));
    }
    match std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
    {
        Ok(()) => {}
        Err(e) => return Err(format!("could not create {shown}: {e}")),
    }
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{shown}: {e}"))?;
    let found = Found {
        symlink: meta.file_type().is_symlink(),
        dir: meta.is_dir(),
        owner: meta.uid(),
        mode: meta.mode(),
    };
    vouch(&shown.to_string(), &found, uid(), shut)?;
    let resolved = std::fs::canonicalize(path).map_err(|e| format!("{shown}: {e}"))?;
    let mut spellings = vec![path.display().to_string()];
    let resolved = resolved.display().to_string();
    if !spellings.contains(&resolved) {
        spellings.push(resolved);
    }
    Ok(spellings)
}

/// What [`prepare`] found at the path, as facts — so each refusal is held
/// by a test without another account to own a directory (review of
/// f50fda5, Reviewers A and C: deleting the owner check, the module's whole
/// premise, passed the suite).
struct Found {
    symlink: bool,
    dir: bool,
    owner: u32,
    mode: u32,
}

/// **Can `found` be vouched for as `euid`'s** — a real directory, not a
/// symlink to one, `euid`'s, and `shut` to every other account? Each refusal
/// says why.
fn vouch(shown: &str, found: &Found, euid: u32, shut: Shut) -> Result<(), String> {
    if found.symlink {
        return Err(format!(
            "{shown} is a symlink — its target is not a place to trust"
        ));
    }
    if !found.dir {
        return Err(format!("{shown} is not a directory"));
    }
    if found.owner != euid {
        return Err(format!(
            "{shown} belongs to another account — anything there may be theirs"
        ));
    }
    if found.mode & shut.mask() != 0 {
        return Err(match shut {
            Shut::Everything => format!(
                "{shown} is open to other accounts (mode {:o}) — `chmod 700` it, or remove it to \
                 have Shannon make it",
                found.mode & 0o777
            ),
            Shut::Writing => format!(
                "{shown} is writable by other accounts (mode {:o}) — `chmod go-w` it",
                found.mode & 0o777
            ),
        });
    }
    Ok(())
}

/// This process's effective user.
fn uid() -> u32 {
    // SAFETY: `geteuid` reads a process attribute; it takes no pointer and
    // cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// **Made owner-only, and used**; both spellings returned.
    #[test]
    fn a_missing_directory_is_made_owner_only() {
        let base = tempfile::TempDir::new().unwrap();
        let path = base.path().join("scratch");
        let spellings = prepare(&path).expect("made");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        assert_eq!(spellings[0], path.display().to_string());
        let resolved = std::fs::canonicalize(&path).unwrap();
        assert!(spellings.contains(&resolved.display().to_string()));
        // Again: an existing one of ours is used as it is.
        assert!(prepare(&path).is_ok());
    }

    /// **Never a place it cannot vouch for** — open to others, a symlink, a
    /// file, a relative name — each said, none used.
    #[test]
    fn a_directory_that_is_not_ours_alone_is_refused() {
        let base = tempfile::TempDir::new().unwrap();
        let open = base.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            prepare(&open)
                .unwrap_err()
                .contains("open to other accounts")
        );

        let link = base.path().join("link");
        let target = base.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(prepare(&link).unwrap_err().contains("symlink"));

        let file = base.path().join("file");
        std::fs::write(&file, "").unwrap();
        assert!(prepare(&file).is_err());

        assert!(prepare(Path::new("relative/x")).is_err());
    }

    /// **Each fact `prepare` holds a directory to refuses on its own** — the
    /// owner above all: a `0700` directory another account made first passes
    /// every other check, and its owner can open it or plant links in it
    /// whenever they like.
    #[test]
    fn each_fact_a_scratch_directory_is_held_to_refuses() {
        let ours = Found {
            symlink: false,
            dir: true,
            owner: 501,
            mode: 0o40700,
        };
        assert!(vouch("d", &ours, 501, Shut::Everything).is_ok(), "premise");
        let cases = [
            (Found { owner: 502, ..ours }, "another account"),
            (Found { dir: false, ..ours }, "not a directory"),
            (
                Found {
                    symlink: true,
                    ..ours
                },
                "symlink",
            ),
            (
                Found {
                    mode: 0o40750,
                    ..ours
                },
                "open to other accounts",
            ),
            (
                Found {
                    mode: 0o40702,
                    ..ours
                },
                "open to other accounts",
            ),
        ];
        for (found, why) in cases {
            let refused = vouch("d", &found, 501, Shut::Everything).expect_err(why);
            assert!(refused.contains(why), "{refused}");
        }
    }

    /// **A memory place may be read by others, never written** — Claude
    /// Code's are `0755` (Steven, 10-06); a link planted there by another
    /// account takes their write access. Every other fact holds as for the
    /// scratch directory.
    #[test]
    fn a_memory_place_is_shut_to_writing_only() {
        let ours = Found {
            symlink: false,
            dir: true,
            owner: 501,
            mode: 0o40755,
        };
        assert!(vouch("d", &ours, 501, Shut::Writing).is_ok());
        assert!(vouch("d", &ours, 501, Shut::Everything).is_err(), "premise");
        for (found, why) in [
            (
                Found {
                    mode: 0o40775,
                    ..ours
                },
                "writable by other accounts",
            ),
            (
                Found {
                    mode: 0o40757,
                    ..ours
                },
                "writable by other accounts",
            ),
            (Found { owner: 502, ..ours }, "another account"),
            (
                Found {
                    symlink: true,
                    ..ours
                },
                "symlink",
            ),
        ] {
            let refused = vouch("d", &found, 501, Shut::Writing).expect_err(why);
            assert!(refused.contains(why), "{refused}");
        }
        let base = tempfile::TempDir::new().unwrap();
        let open = base.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(prepare_owned(&open).is_ok());
        assert!(prepare(&open).is_err());
    }

    /// The default is this user's.
    #[test]
    fn the_default_is_per_user() {
        assert_eq!(
            default_path(),
            PathBuf::from(format!("/tmp/shannon-{}", uid()))
        );
    }
}
