//! **Which directories' own settings apply** — Shannon's trust question,
//! asked once per directory (Steven, 10-04). Not what a command may touch
//! (the rules and the inside default say that, trusted or not): whether
//! the files a repository carries may configure what runs.
//!
//! `claude -p` trusts the directory it is started in without asking: the
//! project's own Claude Code settings apply — its hooks run (a `SessionStart`
//! hook before the first prompt), its `.mcp.json` servers start, its
//! `disableAllHooks` would drop Shannon's gate (`docs/DesignDocs/
//! claude-code-backend.md`, open question 8). Run interactively, Claude Code
//! asks first; Shannon runs it with `-p`, so Shannon asks instead. And
//! Shannon's own files in a repository — all of a `.shannon/`: its
//! `config.toml`, whose rules are yours, its `models/`, its `skills/` — are
//! read only where the directory holding it is trusted (6d; Steven, 10-13:
//! "Trust should gate `.shannon/*`"): a cloned repository's `allow :* /` is
//! not yours by being cloned. Three doors, one answer each:
//! [`Trusted::project_file`], [`Trusted::project_models`],
//! [`Trusted::untrusted_shannon`].
//!
//! A trusted directory covers what is under it, as Claude Code's does. Kept
//! one absolute path a line in Shannon's state directory, owner-only — which
//! the gate protects as Shannon's own state, so an agent cannot write itself a
//! trust — with what each trusted project's file was last said to change
//! about safety ([`Trusted::told`]), so it is not said at every launch.

use std::path::{Path, PathBuf};

/// The file trust is kept in: `$XDG_STATE_HOME/shannon/trusted_dirs`.
pub fn default_path() -> Option<PathBuf> {
    shannon_core::Config::state_dir().map(|dir| dir.join("trusted_dirs"))
}

/// The directories trusted, and where they are kept — `None`: in memory only
/// (a test app; a machine with no state directory).
#[derive(Debug, Clone, Default)]
pub struct Trusted {
    file: Option<PathBuf>,
    dirs: Vec<PathBuf>,
    /// Trusted until Shannon quits — "this time only" — and never written.
    this_run: Vec<PathBuf>,
    /// **What each trusted project's file was last said to change about
    /// safety** — its file, and the stack's digest of those settings
    /// (`ConfigStack::told`) — so launch says it when it is news, not every
    /// time (Steven, 10-19). Kept in the same file, a `told <digest> <path>`
    /// line each.
    told: Vec<(PathBuf, String)>,
}

/// The word a [`Trusted::told`] line starts with in the kept file.
const TOLD: &str = "told ";

/// **How long a trust lasts** — the trust screen's two yeses (Steven, 10-04:
/// "yes (remember), one time, and no").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lasting {
    /// Kept: every launch here from now on.
    Remembered,
    /// Until Shannon quits; the next launch asks again.
    ThisRun,
}

impl Trusted {
    /// **Read from `file`**, or why not — a file that cannot be read is said,
    /// and trusts nothing: an unread trust must not read as one.
    pub fn load(file: Option<PathBuf>) -> (Trusted, Option<String>) {
        let Some(path) = file.clone() else {
            return (Trusted::default(), None);
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                // Untrimmed: a line is a path to its end, and a name may end
                // in a space (`lines` already takes a `\r\n`'s `\r`).
                let lines = || text.lines();
                let dirs = lines()
                    .filter(|l| l.starts_with('/'))
                    .map(PathBuf::from)
                    .collect();
                let told = lines()
                    .filter_map(|l| l.strip_prefix(TOLD)?.split_once(' '))
                    .map(|(digest, path)| (PathBuf::from(path), digest.to_string()))
                    .collect();
                (
                    Trusted {
                        file,
                        dirs,
                        this_run: Vec::new(),
                        told,
                    },
                    None,
                )
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                Trusted {
                    file,
                    ..Trusted::default()
                },
                None,
            ),
            Err(e) => (
                Trusted {
                    file,
                    ..Trusted::default()
                },
                Some(format!(
                    "the trusted directories ({}) could not be read ({e}) — Claude Code will ask \
                     again",
                    path.display()
                )),
            ),
        }
    }

    /// The directories trusted, as kept.
    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }

    /// The directories trusted until Shannon quits.
    pub fn this_run(&self) -> &[PathBuf] {
        &self.this_run
    }

    /// **Is `dir` trusted** — it, or a directory above it, kept or for this
    /// run? Compared as the filesystem resolves it, so a link to a trusted
    /// directory is that directory, and a link OUT of one is not in it.
    pub fn covers(&self, dir: &Path) -> bool {
        let Ok(dir) = dir.canonicalize() else {
            return false;
        };
        self.dirs
            .iter()
            .chain(&self.this_run)
            .any(|trusted| dir.starts_with(trusted))
    }

    /// **The project's config Shannon reads where it `start`ed** —
    /// `<start>/.shannon/config.toml` — only where the start is trusted:
    /// a repository's file is a layer of the config, and its rules are the
    /// user's own (Steven, 10-13), so a directory not trusted gives none.
    pub fn project_file(&self, start: &Path) -> Option<PathBuf> {
        self.covers(start)
            .then(|| shannon_core::config::project_path(start))
    }

    /// **The project's models Shannon reads where it `start`ed** —
    /// `<start>/.shannon/models/` — only where the start is trusted
    /// (`App::load_models`). It was `covers` typed in place there, and
    /// deleting it passed every test (review 2026-10-13, A, B, C).
    pub fn project_models(&self, start: &Path) -> Option<PathBuf> {
        self.covers(start)
            .then(|| shannon_core::config::project_models_dir(start))
    }

    /// **What a start not trusted holds that a trusted one's would apply**
    /// — what its `.shannon/` holds ([`project_held`]) — as the
    /// sentence launch says, so a project's file is never silently not
    /// applied (Steven, 10-13). The trust screen asks only for Claude Code;
    /// on the API this is how you learn of it.
    pub fn unread_project(&self, start: &Path) -> Option<String> {
        let (names, is) = self.unread_names(start)?;
        let them = match is {
            "is" => "it",
            _ => "them",
        };
        Some(format!(
            "this directory's {names} {is} not read — it is not trusted (`:trust .` reads {them})"
        ))
    }

    /// **What a start not trusted holds, unread, as a phrase** — its
    /// `.shannon/` entries listed ([`project_held`]) and the verb they take
    /// ("is"/"are") — for a sentence to lead with: the trust screen's no
    /// (`App::answer_trust`) and [`unread_project`](Self::unread_project).
    pub fn unread_names(&self, start: &Path) -> Option<(String, &'static str)> {
        if self.covers(start) {
            return None;
        }
        let held = project_held(start);
        let is = match held.len() {
            0 => return None,
            1 => "is",
            _ => "are",
        };
        Some((listed(&held), is))
    }

    /// **The directory holding `path`'s `.shannon/`, where it is not
    /// trusted** — the nearest `.shannon` above `path` decides. Shannon
    /// reads a `.shannon/` only where it trusts the directory holding it
    /// (Steven, 10-13: "Trust should gate `.shannon/*`"): its config and
    /// models at launch, and a memory or skill location under one — the
    /// default `.shannon/skills` a repository's walk finds — at every read.
    pub fn untrusted_shannon(&self, path: &Path) -> Option<PathBuf> {
        let dot = path.ancestors().find(|a| {
            a.file_name()
                .is_some_and(|n| n == shannon_core::config::PROJECT_DIR)
        })?;
        let holder = dot.parent()?;
        (!self.covers(holder)).then(|| holder.to_path_buf())
    }

    /// **Trust `dir`** for as long as `lasting` says — kept, or until Shannon
    /// quits. Resolved first, so what is trusted is the directory, not one
    /// spelling of it.
    pub fn trust(&mut self, dir: &Path, lasting: Lasting) -> Result<PathBuf, String> {
        let dir = dir
            .canonicalize()
            .map_err(|e| format!("{} cannot be trusted ({e})", dir.display()))?;
        match lasting {
            // Kept only if it could be kept: trusted in memory with the file
            // unwritten, the models, skills and Claude Code followed the
            // trust while the project's config did not (review 2026-10-13, A).
            Lasting::Remembered if !self.dirs.contains(&dir) => {
                self.dirs.push(dir.clone());
                if let Err(why) = self.save() {
                    self.dirs.pop();
                    return Err(why);
                }
            }
            Lasting::ThisRun if !self.this_run.contains(&dir) => self.this_run.push(dir.clone()),
            _ => {}
        }
        Ok(dir)
    }

    /// **What `file` — a project's config — was last said to change about
    /// safety**, as the stack's digest: what launch hands
    /// `ConfigStack::load_reporting`, so an unchanged file is not said again.
    pub fn told(&self, file: &Path) -> Option<&str> {
        self.told
            .iter()
            .find(|(told, _)| shannon_core::config::same_file(told, file))
            .map(|(_, digest)| digest.as_str())
    }

    /// **Keep that `file` was said to change safety settings as `digest`
    /// says** — after the stack said it (`App::keep_told`) — or why it could
    /// not be kept, and then it is not, as `trust` keeps only what it can
    /// write. Written only under a REMEMBERED trust (`save`); under a trust
    /// for this run it lasts as that trust does.
    pub fn tell(&mut self, file: &Path, digest: &str) -> Result<(), String> {
        let was = self.told.clone();
        self.told
            .retain(|(told, _)| !shannon_core::config::same_file(told, file));
        self.told.push((file.to_path_buf(), digest.to_string()));
        self.save().inspect_err(|_| self.told = was)
    }

    /// **Stop trusting** every directory that covers `dir` — kept or for this
    /// run — and keep that. What was removed, which may be nothing. What was
    /// told of a project's file under one goes with it: trusted again, its
    /// safety note is said again.
    pub fn revoke(&mut self, dir: &Path) -> Result<Vec<PathBuf>, String> {
        let dir = resolved(dir);
        let covering = |dirs: &mut Vec<PathBuf>| -> Vec<PathBuf> {
            let (gone, kept) = std::mem::take(dirs)
                .into_iter()
                .partition(|trusted| dir.starts_with(trusted));
            *dirs = kept;
            gone
        };
        let (kept, this_run, told) = (self.dirs.clone(), self.this_run.clone(), self.told.clone());
        let gone = covering(&mut self.dirs);
        let gone_now = covering(&mut self.this_run);
        // Told of under either kind of trust: a trust for this run left its
        // project's line in memory, and a remembered parent's next launch
        // skipped the note (review stop 6, B 3).
        let all_gone: Vec<&PathBuf> = gone.iter().chain(&gone_now).collect();
        self.told
            .retain(|(file, _)| !all_gone.iter().any(|g| file.starts_with(g)));
        if !gone.is_empty()
            && let Err(why) = self.save()
        {
            // Not revoked if the revoke could not be kept (see `trust`).
            (self.dirs, self.this_run, self.told) = (kept, this_run, told);
            return Err(why);
        }
        Ok(gone.into_iter().chain(gone_now).collect())
    }

    /// **Write the list, owner-only, replacing the file whole** — the one
    /// door to the file, so the one place a path must be one line: a name
    /// holding a line break would be two, and `load` reads any line starting
    /// `/` as a trusted directory (a project named `x\n/\n` behind a `told`
    /// line trusted `/`, review stop 6, B 1). What was told is written only
    /// under a remembered trust: one for this run is never written (`Lasting`).
    fn save(&self) -> Result<(), String> {
        let line = |p: &Path| match p.to_str() {
            Some(s) if !s.contains(['\n', '\r']) => Ok(s.to_string()),
            _ => Err(format!("{} cannot be kept as one line", p.display())),
        };
        let mut text = String::new();
        for dir in &self.dirs {
            text.push_str(&format!("{}\n", line(dir)?));
        }
        let remembered = |file: &Path| self.dirs.iter().any(|d| file.starts_with(d));
        for (file, digest) in self.told.iter().filter(|(f, _)| remembered(f)) {
            text.push_str(&format!("{TOLD}{digest} {}\n", line(file)?));
        }
        // Checked whether or not there is a file to write: in memory too, a
        // trust is only what could be kept.
        let Some(path) = &self.file else {
            return Ok(());
        };
        crate::private_file::replace(path, &text)
            .map_err(|e| format!("the trusted directories were not saved — {e:#}"))
    }
}

/// How many of a `.shannon/`'s entries [`project_held`] names before "and
/// … more", and the most it reads — so a repository's directory of a
/// million files costs a launch nothing.
const HELD_SHOWN: usize = 4;
const HELD_READ: usize = 64;

/// The entries Shannon reads by name, named first wherever they sort.
const HELD_KNOWN: &[&str] = &["config.toml", "models/", "skills/"];

/// **What of `start`'s own `.shannon/` a trust would apply** — every entry
/// there, a directory ending in `/`, those Shannon reads by name first
/// (`HELD_KNOWN`), then the rest sorted: Shannon reads all of it only
/// where it trusts the directory that holds it (Steven, 10-13: "Trust
/// should gate `.shannon/*`"). Past `HELD_SHOWN` the rest is a count —
/// "more", uncounted, where the read stopped at `HELD_READ`: a count
/// taken of the first 64 was said as the whole (review 2026-10-13, C).
/// The trust screen lists them (`Setup::trust`); the "not read" note names
/// them ([`Trusted::unread_project`]).
pub fn project_held(start: &Path) -> Vec<String> {
    let dir = start.join(shannon_core::config::PROJECT_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut read = 0;
    let mut names: Vec<String> = entries
        .take(HELD_READ + 1)
        .inspect(|_| read += 1)
        .filter_map(Result::ok)
        .map(|entry| {
            let slash = match entry.file_type().is_ok_and(|t| t.is_dir()) {
                true => "/",
                false => "",
            };
            format!("{}{slash}", entry.file_name().to_string_lossy())
        })
        .collect();
    let capped = read > HELD_READ;
    names.truncate(HELD_READ);
    names.sort_by_key(|name| {
        let known = HELD_KNOWN.iter().position(|k| k == name);
        (known.unwrap_or(HELD_KNOWN.len()), name.clone())
    });
    let mut names: Vec<String> = names
        .into_iter()
        .map(|name| format!("{}/{name}", shannon_core::config::PROJECT_DIR))
        .collect();
    if names.len() > HELD_SHOWN || capped {
        let more = names.len() - (HELD_SHOWN - 1).min(names.len());
        names.truncate(HELD_SHOWN - 1);
        names.push(match capped {
            true => "more".to_string(),
            false => format!("{more} more"),
        });
    }
    names
}

/// `a`, `a and b`, `a, b and c` — the trust screen's names, and what a delete
/// says went with a session (`App::delete_episodes`).
pub(crate) fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// **`path` as the filesystem resolves it, as far as it exists** — the
/// deepest existing ancestor resolved, the rest kept as written. A path that
/// does not exist yet still lies under the directory it names: unresolved,
/// `/var/…` never met the `/private/var/…` it was kept as on macOS.
fn resolved(path: &Path) -> PathBuf {
    path.ancestors()
        .find_map(|ancestor| {
            let real = ancestor.canonicalize().ok()?;
            let rest = path.strip_prefix(ancestor).ok()?;
            Some(real.join(rest))
        })
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// **A trusted directory covers what is under it, and only that** — a
    /// link to it is it; a sibling with a common prefix is not.
    #[test]
    fn a_trusted_directory_covers_what_is_under_it() {
        let root = TempDir::new().unwrap();
        let project = root.path().join("proj");
        let inner = project.join("src");
        let sibling = root.path().join("proj-other");
        for dir in [&inner, &sibling] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&project, &link).unwrap();
        let mut trusted = Trusted::default();
        assert!(!trusted.covers(&project));
        trusted.trust(&project, Lasting::Remembered).unwrap();
        assert!(trusted.covers(&project));
        assert!(trusted.covers(&inner));
        assert!(trusted.covers(&link), "a link to it is it");
        assert!(!trusted.covers(&sibling), "a common prefix is not a parent");
        assert!(!trusted.covers(root.path()));
    }

    /// **An untrusted directory loads no project file** (6d): the project's
    /// `.shannon/config.toml` is read only where the start is trusted, or
    /// lies below a trusted directory.
    #[test]
    fn an_untrusted_directory_loads_no_project_file() {
        let root = TempDir::new().unwrap();
        let project = root.path().join("proj");
        let inner = project.join("src");
        std::fs::create_dir_all(&inner).unwrap();
        let mut trusted = Trusted::default();
        assert_eq!(trusted.project_file(&project), None);
        trusted.trust(&project, Lasting::ThisRun).unwrap();
        assert_eq!(
            trusted.project_file(&project),
            Some(project.join(".shannon/config.toml"))
        );
        assert_eq!(
            trusted.project_file(&inner),
            Some(inner.join(".shannon/config.toml")),
            "the start's own, below a trusted directory"
        );
    }

    /// **A start not trusted says what it holds unread** — so a project's
    /// file is never silently not applied — and nothing when there is
    /// nothing, or when the start is trusted and it is read.
    #[test]
    fn an_untrusted_start_says_what_it_does_not_read() {
        let root = TempDir::new().unwrap();
        let mut trusted = Trusted::default();
        assert_eq!(trusted.unread_project(root.path()), None, "nothing there");
        let dir = root.path().join(".shannon");
        std::fs::create_dir_all(dir.join("models")).unwrap();
        let said = trusted.unread_project(root.path()).unwrap();
        assert!(
            said.contains(".shannon/models/ is not read") && said.contains("`:trust .` reads it"),
            "{said}"
        );
        std::fs::write(dir.join("config.toml"), "").unwrap();
        let said = trusted.unread_project(root.path()).unwrap();
        assert!(
            said.contains(".shannon/config.toml and .shannon/models/ are not read"),
            "{said}"
        );
        // All of `.shannon/`, not a list of the files Shannon knows.
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        let said = trusted.unread_project(root.path()).unwrap();
        assert!(
            said.contains(".shannon/config.toml, .shannon/models/ and .shannon/skills/ are"),
            "{said}"
        );
        trusted.trust(root.path(), Lasting::ThisRun).unwrap();
        assert_eq!(trusted.unread_project(root.path()), None, "trusted: read");
    }

    /// **A trust or a revoke that cannot be kept is not made** (review
    /// 2026-10-13, A): in memory with the file unwritten, the models,
    /// skills and Claude Code followed it while the project's config did
    /// not.
    #[test]
    fn a_trust_that_cannot_be_kept_is_not_made() {
        use std::os::unix::fs::PermissionsExt;
        let state = TempDir::new().unwrap();
        let file = state.path().join("trusted_dirs");
        let (kept, other) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let (mut trusted, _) = Trusted::load(Some(file));
        let dir = trusted.trust(kept.path(), Lasting::Remembered).unwrap();
        // Trusted for this run as well, so the revoke takes from both lists.
        trusted.trust(kept.path(), Lasting::ThisRun).unwrap();
        let config = dir.join(".shannon/config.toml");
        trusted.tell(&config, "abc").unwrap();
        let mode = |m| std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(m));
        mode(0o500).unwrap();
        let refused = trusted.trust(other.path(), Lasting::Remembered);
        let revoked = trusted.revoke(kept.path());
        mode(0o700).unwrap();
        assert!(
            refused.is_err() && revoked.is_err(),
            "premise: the file cannot be written"
        );
        assert!(!trusted.covers(other.path()), "not trusted");
        assert!(trusted.covers(kept.path()), "not revoked");
        // What was told of it stays too (review stop 6, C 9: the restore was
        // unpinned).
        assert_eq!(trusted.told(&config), Some("abc"), "nor forgotten");
        assert_eq!(
            trusted.this_run(),
            std::slice::from_ref(&dir),
            "nor its trust for this run (its restore was unpinned)"
        );
    }

    /// **What was told of a path with spaces reads back whole** — the line is
    /// `told <digest> <path>`, the digest first and the path the rest, so a
    /// space in the path, or one ending it, is the path's (review stop 6,
    /// A 3: splitting at the last space passed every test).
    #[test]
    fn what_was_told_of_a_spaced_path_reads_back_whole() {
        let root = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let file = state.path().join("trusted_dirs");
        let spaced = root.path().join("My Projects ");
        std::fs::create_dir(&spaced).unwrap();
        let (mut trusted, _) = Trusted::load(Some(file.clone()));
        let dir = trusted.trust(&spaced, Lasting::Remembered).unwrap();
        let config = dir.join(".shannon/config.toml");
        trusted.tell(&config, "abc").unwrap();
        let (again, _) = Trusted::load(Some(file));
        assert_eq!(again.dirs(), [dir], "a trailing space is the path's");
        assert_eq!(again.told(&config), Some("abc"));
    }

    /// **What a `.shannon/` holds, named honestly** — what Shannon reads by
    /// name first, then the rest, and past the read's bound "more",
    /// uncounted: a count of the first 64 was said as the whole (review
    /// 2026-10-13, C).
    #[test]
    fn what_a_shannon_dir_holds_is_named_honestly() {
        let root = TempDir::new().unwrap();
        let dir = root.path().join(".shannon");
        std::fs::create_dir_all(dir.join("models")).unwrap();
        std::fs::write(dir.join("config.toml"), "").unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();
        assert_eq!(
            project_held(root.path()),
            [".shannon/config.toml", ".shannon/models/", ".shannon/a.txt"],
            "known first"
        );
        for n in 0..70 {
            std::fs::write(dir.join(format!("f{n:03}")), "").unwrap();
        }
        let held = project_held(root.path());
        assert_eq!(held.len(), HELD_SHOWN, "{held:?}");
        assert_eq!(held.last().map(String::as_str), Some("more"), "{held:?}");
    }

    /// **Kept, owner-only, and read back** — and a revoke is kept too.
    #[test]
    fn trust_is_kept_owner_only_and_revoked() {
        use std::os::unix::fs::PermissionsExt;
        let state = TempDir::new().unwrap();
        crate::private_file::assert_umask_grants_more(state.path());
        let file = state.path().join("trusted_dirs");
        let project = TempDir::new().unwrap();
        let (mut trusted, note) = Trusted::load(Some(file.clone()));
        assert_eq!(note, None, "no file yet is no trust, and nothing to say");
        trusted.trust(project.path(), Lasting::Remembered).unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let (again, _) = Trusted::load(Some(file.clone()));
        assert!(again.covers(project.path()), "read back");
        let gone = trusted.revoke(&project.path().join("deeper")).unwrap();
        assert_eq!(gone.len(), 1, "a revoke from inside removes what covers it");
        let (after, _) = Trusted::load(Some(file));
        assert!(!after.covers(project.path()));
    }

    /// **A directory whose name holds a line break is not trusted** — kept
    /// one path a line, it would be kept as two; one ending in `\r` would be
    /// read back as another name (`lines` takes a `\r` before the `\n`).
    #[test]
    fn a_name_that_would_be_two_lines_is_refused() {
        let root = TempDir::new().unwrap();
        for name in ["a\nb", "c\r"] {
            let odd = root.path().join(name);
            std::fs::create_dir(&odd).unwrap();
            let mut trusted = Trusted::default();
            assert!(
                trusted.trust(&odd, Lasting::Remembered).is_err(),
                "{name:?}"
            );
            assert!(trusted.dirs().is_empty());
        }
    }

    /// **What was told of a file named over two lines is refused too** —
    /// `load` reads any line starting `/` as a trusted directory, so a
    /// project named `x\n/\n` under a trusted parent trusted `/` the next
    /// launch (review stop 6, B 1: the `tell` guard was its own copy, and its
    /// deletion passed every test). One door now: `save`.
    #[test]
    fn a_told_file_named_over_two_lines_trusts_nothing_more() {
        let root = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let file = state.path().join("trusted_dirs");
        let (mut trusted, _) = Trusted::load(Some(file.clone()));
        let parent = trusted.trust(root.path(), Lasting::Remembered).unwrap();
        let odd = parent.join("x\n/\n/.shannon/config.toml");
        assert!(trusted.tell(&odd, "abc").is_err(), "refused");
        assert_eq!(trusted.told(&odd), None, "and not kept in memory either");
        let (again, _) = Trusted::load(Some(file));
        assert_eq!(
            again.dirs(),
            std::slice::from_ref(&parent),
            "trusts nothing more"
        );
        assert!(!again.covers(Path::new("/usr")));
    }

    /// **What was told under a trust for this run is never written, and goes
    /// with `:untrust`** (review stop 6, B 3) — "this time only" left the
    /// project's path on disk, and a remembered parent's later launch skipped
    /// the note.
    #[test]
    fn what_was_told_under_a_trust_for_this_run_is_never_written() {
        let root = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let file = state.path().join("trusted_dirs");
        let (mut trusted, _) = Trusted::load(Some(file.clone()));
        let dir = trusted.trust(root.path(), Lasting::ThisRun).unwrap();
        let config = dir.join(".shannon/config.toml");
        trusted.tell(&config, "abc").unwrap();
        assert_eq!(trusted.told(&config), Some("abc"), "kept for the run");
        let on_disk = std::fs::read_to_string(&file).unwrap_or_default();
        assert!(!on_disk.contains("told"), "never written: {on_disk:?}");
        trusted.revoke(&dir).unwrap();
        assert_eq!(trusted.told(&config), None, "gone with the trust");
    }

    /// **A trust file that cannot be read trusts nothing, and says so.**
    #[test]
    fn an_unreadable_trust_file_trusts_nothing() {
        let state = TempDir::new().unwrap();
        // A directory where the file should be: present, and unreadable as one.
        let file = state.path().join("trusted_dirs");
        std::fs::create_dir(&file).unwrap();
        let (trusted, note) = Trusted::load(Some(file));
        assert!(trusted.dirs().is_empty());
        assert!(note.is_some_and(|n| n.contains("could not be read")));
    }

    /// **"This time only" trusts now and is never written** — the next
    /// launch reads nothing and asks again; `:untrust` takes it back too.
    #[test]
    fn a_trust_for_this_run_is_never_written() {
        let state = TempDir::new().unwrap();
        let file = state.path().join("trusted_dirs");
        let project = TempDir::new().unwrap();
        let (mut trusted, _) = Trusted::load(Some(file.clone()));
        trusted.trust(project.path(), Lasting::ThisRun).unwrap();
        assert!(trusted.covers(project.path()), "trusted now");
        assert!(!file.exists(), "and nothing written");
        let (next_launch, _) = Trusted::load(Some(file));
        assert!(!next_launch.covers(project.path()), "the next launch asks");
        assert_eq!(trusted.revoke(project.path()).unwrap().len(), 1);
        assert!(!trusted.covers(project.path()), "revoked");
    }
}
