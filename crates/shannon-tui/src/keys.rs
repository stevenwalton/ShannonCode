//! **Where Shannon finds its API key** — the rule launch reads by
//! (`main::build_backend`), setup states (`app::setup_flow`), and the gate
//! protects every file of (`main::app_config`).
//!
//! One module because it was two copies: the rule lived in `main.rs` — a
//! binary no test imports — and setup kept a hand-written list of the
//! variables that come first, and the copies already disagreed: launch takes
//! a SET-but-empty `SHANNON_API_KEY_FILE` as a named file, setup took it as
//! unset, so setup saved a key without saying launch would never read it
//! (review 21, Reviewer C).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use shannon_core::config::CacheConfig;
use shannon_core::{Backend, Config};

// In core, beside the one list of them (`KEY_VARS`) that `claude` is never
// handed.
pub use shannon_core::config::{API_KEY_VAR, KEY_FILE_VAR};

/// **Where the key the API answers with came from** — what setup says of the
/// key in use, and how it knows whether saving it would change anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyFrom {
    /// An environment variable, by name.
    Var(&'static str),
    /// A key file.
    File(PathBuf),
    /// Pasted into setup this session, and not saved.
    Pasted,
}

impl std::fmt::Display for KeyFrom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyFrom::Var(name) => write!(f, "{name}"),
            KeyFrom::File(path) => write!(f, "{}", path.display()),
            KeyFrom::Pasted => write!(f, "pasted in setup, not saved"),
        }
    }
}

/// Where a key file came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// `SHANNON_API_KEY_FILE`: the user named it, so it is read alone.
    Named,
    /// The key `shannon setup` saved (`Config::api_key_path`).
    Saved,
}

/// The file `SHANNON_API_KEY_FILE` names, when it is set — empty included,
/// because launch reads a named file alone.
pub fn named_key_file(var: Option<std::ffi::OsString>) -> Option<PathBuf> {
    var.map(PathBuf::from)
}

/// **Every key file, in reading order** — the named file, then the saved key.
/// A named file is read alone ([`find_key`] stops at it, found or not), but
/// the saved key is still listed: it is a credential this process writes or
/// could read, and is protected all the same.
///
/// **No file is looked for where Shannon starts.** A `./.test_key.api` was,
/// a dev convenience from the start of the project: a name nobody would
/// guess, read from whatever directory Shannon ran in — so a repository
/// could supply the key — and, guarded relative and absent, it made every
/// read of the start ask (Steven's test, 10-11). Removed (Steven: "cruft
/// left over from when we started"); `SHANNON_API_KEY_FILE` names a file.
///
/// The env-free half of [`key_files_for`], so a test drives the real answer.
pub fn key_files_from(named: Option<PathBuf>, saved: Option<PathBuf>) -> Vec<(PathBuf, KeySource)> {
    named
        .map(|p| (p, KeySource::Named))
        .into_iter()
        .chain(saved.map(|p| (p, KeySource::Saved)))
        .collect()
}

/// [`key_files_from`] as this process's environment says.
///
/// **One function, because the answer has two consumers**: launch reads the key
/// from here, and registers every one of these paths as sensitive so the agent
/// cannot read a credential this process could authenticate with
/// ([`protected_key_files`]). That registration was lost once (2026-08-11)
/// and nothing caught it, and again under review (stop 12, T6 2).
pub fn key_files_for() -> Vec<(PathBuf, KeySource)> {
    key_files_from(
        named_key_file(std::env::var_os(KEY_FILE_VAR)),
        Config::api_key_path(),
    )
}

/// **Every key file as the gate guards it: where it is** — the paths launch
/// hands the gate (`main::app_config`, through `App::protect_paths`) for
/// `candidates`, EVERY one, not only the one read: the saved key is
/// Shannon's own credential whether or not a named file is read ahead of it.
///
/// - **Registered, as well as pattern-matched**: `*.api` covers the
///   repo-root convention, and a pattern cannot cover a path the user chose.
/// - **Absolute**: a relative path is made so against `cwd`, where launch
///   reads it from. Guarded relative, a key file stood under `.` in every
///   project, and every read of the start asked with no lift — `ls`, `git
///   status`, `find .`, any unmodelled program (Steven's test, 10-11: the
///   since-removed `./.test_key.api`, absent there).
/// - **Whether or not it exists**: the user chose the named path, and
///   `shannon setup` writes the saved one.
/// - **Where it really is, too** ([`with_real_path`](crate::app::with_real_path)):
///   the gate reads a path as written, so a key named through a link into
///   where Shannon started was guarded by the link's name alone, and read
///   unasked by its own (review stop 12, found fixing T6 2).
///
/// Here, not in `main.rs`, so the composition is tested: launch's loop over
/// the candidates was deleted and every test passed (review stop 12, T6 2).
pub fn protected_key_files(candidates: &[(PathBuf, KeySource)], cwd: Option<&Path>) -> Vec<String> {
    candidates
        .iter()
        .map(|(path, _)| match (path.is_absolute(), cwd) {
            (false, Some(cwd)) => cwd.join(path),
            _ => path.clone(),
        })
        .flat_map(|path| crate::app::with_real_path(&path))
        .map(|path| path.display().to_string())
        .collect()
}

/// **Find the key, or say why there is none** — the whole of launch's rule,
/// with the environment and the filesystem handed in, so every arm is tested
/// (it lived in `main.rs` untested: deleting the named-file rule passed every
/// test; review 21, Reviewers A and C).
///
/// In order: `env_key` if set and non-empty; then each candidate. A file read
/// gives its first line. A NAMED file is read alone — found, empty, or
/// unreadable, nothing is looked at past it. The saved key's absence is "not
/// set up yet" — the ordinary no-key path; any other failure reading it is
/// said.
///
/// `Ok` carries the key and where it came from — for the log, and for setup
/// to say and to save.
pub fn find_key(
    env_key: Option<String>,
    candidates: &[(PathBuf, KeySource)],
    read: impl Fn(&Path) -> std::io::Result<String>,
) -> Result<(String, KeyFrom), StubReason> {
    if let Some(key) = env_key.filter(|k| !k.is_empty()) {
        return Ok((key, KeyFrom::Var(API_KEY_VAR)));
    }
    for (path, source) in candidates {
        let shown = path.display().to_string();
        match read(path) {
            Ok(contents) => {
                let key = contents.lines().next().unwrap_or("").trim().to_string();
                return match key.is_empty() {
                    true => Err(StubReason::Empty(shown)),
                    false => Ok((key, KeyFrom::File(path.clone()))),
                };
            }
            Err(_) if *source == KeySource::Named => return Err(StubReason::Unreadable(shown)),
            Err(e) if *source == KeySource::Saved && e.kind() != std::io::ErrorKind::NotFound => {
                return Err(StubReason::SavedUnreadable(shown));
            }
            Err(_) => continue,
        }
    }
    Err(StubReason::NoKey)
}

/// **Which variable launch reads ahead of a saved key**, if one is set —
/// [`find_key`]'s own order, so setup warns exactly when launch would not
/// reach the key it saves. The key variable counts when non-empty; a key-FILE
/// variable when set at all, empty included, because launch reads it alone.
/// `var` is the environment, handed in. Names only — never a value.
pub fn read_ahead_of_saved(
    var: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Option<&'static str> {
    if var(API_KEY_VAR).is_some_and(|v| !v.is_empty()) {
        return Some(API_KEY_VAR);
    }
    Some(KEY_FILE_VAR).filter(|name| var(name).is_some())
}

/// **The API backend from a key** — what launch builds from a key it finds and
/// setup from one it saves (`App::make_api`), so the two cannot differ.
pub fn api_backend(key: String, cache: CacheConfig) -> anyhow::Result<Arc<dyn Backend>> {
    Ok(Arc::new(shannon_core::backend::AnthropicApi::new(
        key,
        shannon_core::backend::anthropic::DEFAULT_MODEL,
        cache,
    )?))
}

/// Why this process is answering from the canned stub instead of the API.
///
/// A value rather than log lines, so the *sentence the user reads* is a pure
/// function of it and can be tested (`[[feedback_wiring_is_untested]]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StubReason {
    /// No key anywhere — the documented offline path.
    NoKey,
    /// A key file we could read, holding nothing.
    Empty(String),
    /// An explicit `SHANNON_API_KEY_FILE` we could not read at all.
    Unreadable(String),
    /// The key `shannon setup` saved, there and unreadable.
    SavedUnreadable(String),
}

/// The startup note a stub fallback owes the user.
///
/// **Every one of these is a note**, including the ordinary no-key path.
/// Running against the stub is not an error, but it is the single most
/// misleading state the program has: the replies are canned, so "the model is
/// talking nonsense" and "there is no API key" look identical from the
/// transcript.
pub fn stub_note(reason: &StubReason) -> String {
    match reason {
        StubReason::NoKey => "no API key found — answering from the canned stub, not the API \
             (`:setup` connects one)"
            .to_string(),
        StubReason::Empty(path) => {
            format!("key file `{path}` is empty — answering from the canned stub, not the API")
        }
        StubReason::Unreadable(path) => {
            format!(
                "SHANNON_API_KEY_FILE `{path}` could not be read — answering from the canned \
                 stub, not the API"
            )
        }
        StubReason::SavedUnreadable(path) => {
            format!(
                "the key setup saved at `{path}` could not be read — answering from the canned \
                 stub, not the API (`:setup` saves it again)"
            )
        }
    }
}

/// **A first launch** — no key anywhere and no config file — which opens
/// setup by itself (Steven, 10-04). Either one means someone has been here:
/// a key answers, and a config file is a choice already made (setup writes
/// one, even for the demo).
pub fn first_launch(stub: Option<&StubReason>, config_exists: bool) -> bool {
    stub == Some(&StubReason::NoKey) && !config_exists
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every stub fallback names the stub, says what it is NOT talking to,
    /// names the file where there is one, and no two read the same.
    #[test]
    fn every_stub_fallback_says_it_is_a_stub() {
        let cases = [
            StubReason::NoKey,
            StubReason::Empty("/tmp/empty.api".into()),
            StubReason::Unreadable("/opt/creds/missing".into()),
            StubReason::SavedUnreadable("/home/me/.config/shannon/api_key".into()),
        ];
        for reason in &cases {
            let note = stub_note(reason);
            assert!(note.contains("stub"), "{reason:?}: {note}");
            assert!(note.contains("not the API"), "{reason:?}: {note}");
        }
        assert!(stub_note(&cases[1]).contains("/tmp/empty.api"));
        assert!(stub_note(&cases[2]).contains("/opt/creds/missing"));
        assert!(stub_note(&cases[3]).contains("/home/me/.config/shannon/api_key"));
        assert!(stub_note(&cases[0]).contains("`:setup`"));
        let notes: Vec<String> = cases.iter().map(stub_note).collect();
        for (i, a) in notes.iter().enumerate() {
            for b in notes.iter().skip(i + 1) {
                assert_ne!(a, b, "two different causes must not read the same");
            }
        }
    }

    /// Does the gate let `cat` read `path` unasked, given `protected` as
    /// launch hands it over (`App::protect_paths` → `with_own_state`)? Started
    /// at `/`, so every read is inside and allowed by default: only a guard
    /// asks. The gate's own door — not `secrets::sensitive_path`, a glob
    /// matcher beside it that nothing in the gate called, which the test
    /// before this one asked (review stop 12, T6 2; since deleted, Q1).
    fn read_unasked(protected: &[String], path: &str) -> bool {
        let rules = shannon_core::permission::RuleSet::defaults()
            .with_start("/")
            .with_own_state(protected.to_vec());
        rules.decide_line(&format!("cat '{path}'")).decision
            == shannon_core::permission::Decision::Allow
    }

    /// **Every key file is guarded, where it is** — each candidate, the
    /// named one and the saved one, through the list launch hands the gate
    /// ([`protected_key_files`]), a relative one made absolute where launch
    /// reads it; and not its neighbour. Launch's loop over the candidates was
    /// deleted, and every test passed (review stop 12, T6 2).
    #[test]
    fn every_key_file_is_guarded_where_it_is() {
        let cwd = Path::new("/home/me/proj");
        let saved = PathBuf::from("/home/me/.config/shannon/api_key");
        for (named, at) in [
            // A path no shipped pattern covers: the registration is its
            // only protection (`[[feedback_fixture_omits_the_axis]]`).
            (Some("/opt/creds/shannon-token"), "/opt/creds/shannon-token"),
            (Some("keys/prod"), "/home/me/proj/keys/prod"),
            (None, ""),
        ] {
            let candidates = key_files_from(named.map(PathBuf::from), Some(saved.clone()));
            let protected = protected_key_files(&candidates, Some(cwd));
            let want: Vec<&str> = [at, "/home/me/.config/shannon/api_key"]
                .into_iter()
                .filter(|p| !p.is_empty())
                .collect();
            assert_eq!(protected, want, "every candidate, absolute: {named:?}");
            for path in &want {
                assert!(
                    read_unasked(&[], path),
                    "premise: only the guard asks of {path}"
                );
                assert!(
                    !read_unasked(&protected, path),
                    "{path} read unasked ({named:?})"
                );
            }
            assert!(
                read_unasked(&protected, "/opt/creds/notes.md"),
                "a neighbour stays ordinary"
            );
        }
        // No `cwd` (it could not be read): the path as named.
        let named = key_files_from(Some("keys/prod".into()), None);
        assert_eq!(protected_key_files(&named, None), ["keys/prod"]);
    }

    /// **A key file named through a link is guarded where it really is,
    /// too** — the gate reads a path as written, so a key named as
    /// `~/link/k`, with the link leading into where Shannon started, was
    /// guarded by the link's name alone, and `cat secrets/k` from the start
    /// read it unasked: own state and Claude Code's home were registered by
    /// both names (`with_real_path`, review 2026-10-11, E2), the key file
    /// never (review stop 12, found fixing T6 2).
    #[cfg(unix)]
    #[test]
    fn a_key_file_named_through_a_link_is_guarded_where_it_really_is() {
        let dir = tempfile::tempdir().unwrap();
        let start = std::fs::canonicalize(dir.path()).unwrap().join("proj");
        std::fs::create_dir_all(start.join("secrets")).unwrap();
        std::fs::write(start.join("secrets/k"), "sk-x\n").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(start.join("secrets"), &link).unwrap();
        let named = key_files_from(Some(link.join("k")), None);
        let protected = protected_key_files(&named, None);
        let rules = |protected: &[String]| {
            shannon_core::permission::RuleSet::defaults()
                .with_start(&start.display().to_string())
                .with_own_state(protected.to_vec())
        };
        let inside = "cat secrets/k";
        let allow = shannon_core::permission::Decision::Allow;
        assert_eq!(
            rules(&[]).decide_line(inside).decision,
            allow,
            "premise: unguarded, a read inside the start is allowed"
        );
        assert_ne!(
            rules(&protected).decide_line(inside).decision,
            allow,
            "the key, by its real name, read unasked: {protected:?}"
        );
        assert!(
            protected.contains(&link.join("k").display().to_string()),
            "and by the name it was given: {protected:?}"
        );
    }

    /// The order a key file is looked for in, that every one is listed — and
    /// that nothing is looked for where Shannon starts (the removed
    /// `./.test_key.api`).
    #[test]
    fn a_named_key_file_comes_first_then_the_saved_key_and_nothing_else() {
        let saved = PathBuf::from("/home/me/.config/shannon/api_key");
        let sources = |named: Option<PathBuf>| -> Vec<KeySource> {
            key_files_from(named, Some(saved.clone()))
                .into_iter()
                .map(|(_, s)| s)
                .collect()
        };
        assert_eq!(
            sources(Some(PathBuf::from("/opt/creds/token"))),
            [KeySource::Named, KeySource::Saved]
        );
        assert_eq!(sources(None), [KeySource::Saved]);
        assert_eq!(key_files_from(None, None), []);
    }

    /// **Every arm of launch's rule**, through `find_key` with a fake
    /// filesystem: the environment first; a named file alone — found, empty,
    /// or unreadable, never past it; the saved key's absence passing on and
    /// any other failure said; the default's absence the no-key path.
    #[test]
    fn find_key_reads_by_launchs_rule() {
        use std::io::{Error, ErrorKind};
        let named = PathBuf::from("/opt/named");
        let saved = PathBuf::from("/home/me/.config/shannon/api_key");
        let all = key_files_from(Some(named.clone()), Some(saved.clone()));
        let unnamed = key_files_from(None, Some(saved.clone()));
        let read = |files: Vec<(&'static str, std::io::Result<String>)>| {
            move |p: &Path| -> std::io::Result<String> {
                files
                    .iter()
                    .find(|(path, _)| Path::new(path) == p)
                    .map(|(_, r)| match r {
                        Ok(text) => Ok(text.clone()),
                        Err(e) => Err(Error::from(e.kind())),
                    })
                    .unwrap_or_else(|| Err(ErrorKind::NotFound.into()))
            }
        };
        let none = || read(vec![]);
        assert_eq!(
            find_key(Some("sk-env".into()), &all, none()),
            Ok(("sk-env".into(), KeyFrom::Var(API_KEY_VAR)))
        );
        assert_eq!(
            find_key(Some(String::new()), &unnamed, none()),
            Err(StubReason::NoKey),
            "an empty variable is no key"
        );
        let saved_too = read(vec![
            ("/opt/named", Err(ErrorKind::NotFound.into())),
            ("/home/me/.config/shannon/api_key", Ok("sk-saved\n".into())),
        ]);
        assert_eq!(
            find_key(None, &all, saved_too),
            Err(StubReason::Unreadable("/opt/named".into())),
            "a named file is read alone: never past it to the saved key"
        );
        let empty = read(vec![("/opt/named", Ok("\n".into()))]);
        assert_eq!(
            find_key(None, &all, empty),
            Err(StubReason::Empty("/opt/named".into()))
        );
        let saved_only = read(vec![(
            "/home/me/.config/shannon/api_key",
            Ok("sk-saved\n".into()),
        )]);
        assert_eq!(
            find_key(None, &unnamed, saved_only),
            Ok(("sk-saved".into(), KeyFrom::File(saved.clone())))
        );
        let denied = read(vec![(
            "/home/me/.config/shannon/api_key",
            Err(ErrorKind::PermissionDenied.into()),
        )]);
        assert_eq!(
            find_key(None, &unnamed, denied),
            Err(StubReason::SavedUnreadable(saved.display().to_string()))
        );
        assert_eq!(
            find_key(None, &unnamed, none()),
            Err(StubReason::NoKey),
            "nothing anywhere: the no-key path, which a first launch opens setup on"
        );
    }

    /// **Setup's warning follows launch's rule**: a set key variable (not an
    /// empty one), and a key-FILE variable set at all — empty included,
    /// because launch reads a named file alone and an empty name is named.
    #[test]
    fn what_is_read_ahead_of_a_saved_key_is_launchs_rule() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| std::ffi::OsString::from(v))
            }
        };
        assert_eq!(read_ahead_of_saved(env(&[])), None);
        assert_eq!(read_ahead_of_saved(env(&[(API_KEY_VAR, "")])), None);
        assert_eq!(
            read_ahead_of_saved(env(&[(API_KEY_VAR, "k")])),
            Some(API_KEY_VAR)
        );
        assert_eq!(
            read_ahead_of_saved(env(&[(KEY_FILE_VAR, "")])),
            Some(KEY_FILE_VAR)
        );
        // And launch agrees: an empty named file is named, so read alone.
        let named = named_key_file(Some(std::ffi::OsString::new()));
        assert_eq!(named, Some(PathBuf::from("")));
    }

    /// **Setup opens by itself only on a first launch.**
    #[test]
    fn only_a_first_launch_opens_setup_by_itself() {
        assert!(first_launch(Some(&StubReason::NoKey), false));
        assert!(
            !first_launch(Some(&StubReason::NoKey), true),
            "a config file"
        );
        assert!(!first_launch(None, false), "a key answers");
        for there in [
            StubReason::Empty("k".into()),
            StubReason::Unreadable("k".into()),
            StubReason::SavedUnreadable("k".into()),
        ] {
            assert!(!first_launch(Some(&there), false), "{there:?}");
        }
    }
}
