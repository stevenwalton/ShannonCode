//! **Where a file call's path lands** (F2c, `file-tools.md` §3.5) — the
//! file it opens once every link on the way is followed, when that is not
//! the file its path names.
//!
//! The gate places a path by its spelling and follows no link (a limitation
//! `permission::pipeline` documents), so a link in the project to
//! `~/.ssh/config` is judged as a read inside. A file tool knows the one
//! file it opens, so its reading names both (`files::reading`): the path,
//! then where it lands, each judged and the stricter winning — a landing can
//! make a call ask where it would not have, never the reverse.
//!
//! **The system's own links at the root are no landing**: `/tmp` →
//! `/private/tmp` on macOS, `/bin` → `/usr/bin` on a merged Linux. The gate
//! reads those itself, in the one spelling every path is compared in
//! (`shell::pipeline::normalize_path`), so a path lands elsewhere only when
//! that spelling is not where it really goes. A link further down the way
//! is still followed.

use crate::shell::pipeline::{RootLink, normalize_among, root_links};
use std::path::{Path, PathBuf};

/// How many links one path may pass through before it is a loop — Linux's
/// `MAXSYMLINKS`, above macOS's 32.
const MAX_HOPS: usize = 40;

/// **Where `path` lands, if not on the file it names** — `Ok(None)` when it
/// is that file, or when where it lands cannot be found (a Read of nothing,
/// a directory not there: the call fails on its own). An error when it lands
/// on a file whose name is not text, which the rules cannot be asked about.
/// A relative path is from the process's directory, as the call opening it
/// takes it.
pub fn lands(path: &Path) -> Result<Option<PathBuf>, String> {
    lands_among(path, root_links())
}

fn lands_among(path: &Path, links: &[RootLink]) -> Result<Option<PathBuf>, String> {
    // Absolute but not resolved: what the path names, to set beside where
    // it lands.
    let Ok(given) = std::path::absolute(path) else {
        return Ok(None);
    };
    let Some(real) = followed(&given, MAX_HOPS) else {
        return Ok(None);
    };
    // Compared with what the gate reads the path as: a landing is exactly
    // where that reading is not the file opened. A name that is not text
    // is no reading of the gate's, and stays as given.
    let read = given
        .to_str()
        .map_or(given.clone(), |g| normalize_among(g, links).into());
    if real.components().eq(read.components()) {
        return Ok(None);
    }
    match real.to_str() {
        Some(_) => Ok(Some(real)),
        None => Err(format!(
            "`{}` leads to a file whose name is not text, which Shannon cannot name to its rules",
            path.display()
        )),
    }
}

/// **`path` with every link followed** — through a link to nothing too,
/// which a Write follows and makes. `None` past `hops` links, or where no
/// directory on the way is there.
fn followed(path: &Path, hops: usize) -> Option<PathBuf> {
    if let Ok(real) = std::fs::canonicalize(path) {
        return Some(real);
    }
    let hops = hops.checked_sub(1)?;
    let link = std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink());
    if link {
        // A relative target is from the link's own directory; an absolute
        // one replaces the path whole (`join`).
        let to = std::fs::read_link(path).ok()?;
        return followed(&path.parent()?.join(to), hops);
    }
    // Not there yet — a Write's new file: its directory's landing, its name.
    Some(followed(path.parent()?, hops)?.join(path.file_name()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// A directory with no link on its way, so a test's expectations are
    /// not the temp directory's (`/var` → `/private/var` on macOS).
    fn real_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(dir.path()).unwrap();
        (dir, real)
    }

    /// **A path lands where its links lead** — through a link to a file, a
    /// link to a directory on the way, a link to nothing (a Write makes its
    /// target), and a new file under a linked directory; on itself, `None`.
    #[test]
    fn a_path_lands_where_its_links_lead() {
        let (_dir, d) = real_dir();
        let (inside, outside) = (d.join("project"), d.join("outside"));
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret"), "x").unwrap();
        std::fs::write(inside.join("plain.txt"), "x").unwrap();
        symlink(outside.join("secret"), inside.join("notes")).unwrap();
        symlink(&outside, inside.join("sub")).unwrap();
        // Relative, from the link's directory.
        symlink("../outside/made-by-write", inside.join("dangling")).unwrap();

        let none: &[RootLink] = &[];
        let lands = |p: PathBuf| lands_among(&p, none).unwrap();
        assert_eq!(lands(inside.join("plain.txt")), None);
        assert_eq!(lands(inside.join("new.txt")), None, "a new file, no link");
        assert_eq!(lands(inside.join("notes")), Some(outside.join("secret")));
        assert_eq!(
            lands(inside.join("sub/secret")),
            Some(outside.join("secret"))
        );
        assert_eq!(
            lands(inside.join("sub/new.txt")),
            Some(outside.join("new.txt")),
            "a new file under a linked directory"
        );
        assert_eq!(
            lands(inside.join("dangling")),
            Some(outside.join("made-by-write")),
            "a link to nothing lands where a Write would make it"
        );
        // A `..` the gate reads as the file opened is no landing; through a
        // link it reads elsewhere (the gate's `..` is lexical), and is one.
        assert_eq!(lands(inside.join("../outside/secret")), None);
        symlink(&outside, d.join("elsewhere")).unwrap();
        std::fs::write(d.join("sibling"), "x").unwrap();
        std::fs::write(d.join("outside/../sibling-not"), "x").unwrap();
        assert_eq!(
            lands(d.join("elsewhere/../sibling")),
            None,
            "`elsewhere/..` is `d` both ways"
        );
        symlink(inside.join("sub"), d.join("deep")).unwrap();
        assert_eq!(lands(d.join("deep/secret")), Some(outside.join("secret")));
        assert_eq!(lands(d.join("no/such/dir/file")), None, "not found");
    }

    /// **A loop lands nowhere**, rather than spinning: the hops run out.
    #[test]
    fn a_loop_of_links_lands_nowhere() {
        let (_dir, d) = real_dir();
        symlink(d.join("b"), d.join("a")).unwrap();
        symlink(d.join("a"), d.join("b")).unwrap();
        assert_eq!(lands_among(&d.join("a"), &[]).unwrap(), None);
    }

    /// **A root link is no landing, by either spelling** — the machine's
    /// own, so `/tmp` on macOS and `/bin` on a merged Linux. A name nothing
    /// makes: a new file under the link, as a Write's is. (Compared as
    /// given, every call in a project under `/tmp` reported a landing and
    /// six tests went red there; found by `tools/publish/export.py --test`
    /// and review stop 17, V H2.)
    #[test]
    fn a_root_link_is_no_landing_by_either_spelling() {
        let links = root_links();
        if cfg!(target_os = "macos") {
            assert!(!links.is_empty(), "premise: `/tmp` is one here");
        }
        for (name, to) in links {
            let Ok(real) = std::fs::canonicalize(to) else {
                continue;
            };
            if real.to_str() != Some(to) {
                continue; // an automount's link: the door reads no `stat`
            }
            let new = "shannon-lands-probe-of-nothing";
            for path in [format!("/{name}/{new}"), format!("{to}/{new}")] {
                assert_eq!(lands(Path::new(&path)).unwrap(), None, "{path}");
            }
        }
    }

    /// **The real place is what a landing says**, in the gate's spelling, and
    /// a link further down than the root is still followed.
    #[test]
    fn a_landing_is_named_in_the_gates_spelling() {
        let (_dir, d) = real_dir();
        std::fs::create_dir_all(d.join("home")).unwrap();
        std::fs::create_dir_all(d.join("scratch")).unwrap();
        symlink(d.join("home"), d.join("scratch/escape")).unwrap();
        assert_eq!(
            lands_among(&d.join("scratch/escape/x"), &[]).unwrap(),
            Some(d.join("home/x"))
        );
    }

    /// **A landing whose name is not text is refused** — the rules are
    /// asked about text, and a lossy spelling would judge another file. A
    /// link to nothing by that name, which a Write would make (APFS makes
    /// no such file itself).
    #[test]
    fn a_landing_that_is_not_text_is_refused() {
        use std::os::unix::ffi::OsStrExt;
        let (_dir, d) = real_dir();
        let odd = d.join(std::ffi::OsStr::from_bytes(b"odd-\xff"));
        symlink(&odd, d.join("link")).unwrap();
        let refused = lands_among(&d.join("link"), &[]).unwrap_err();
        assert!(refused.contains("not text"), "{refused}");
    }
}
