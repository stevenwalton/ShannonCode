//! **Where a marked secret waits between runs** — outside every file the agent
//! can read.
//!
//! A session's transcript keeps HANDLES, never values, which is what makes it
//! safe on disk. It also meant a resumed session's handles resolved to
//! nothing: the table died with the process, and a command the model wrote
//! with an old handle refused. Steven, 2026-10-01: "we have to be able to load
//! the secrets back into the session."
//!
//! # Keyed by handle, not by session
//!
//! A secret's handle carries a nonce drawn from a per-run random seed, so a
//! handle is unique across every run there has been. The vault can therefore
//! be a plain map from handle to what it stood for, with no notion of
//! sessions: a secret is **kept** when it is marked, and **recalled** when a
//! history being loaded holds its handle. One door in, one door out; nothing
//! at the four places turns are written.
//!
//! Recall is by the **exact** handle. A history can hold handles nobody
//! minted — `{{github-token.ffff}}` planted in a README the model read — and
//! resolving those by NAME would be precisely the guessable capability the
//! nonce exists to prevent. Exactness alone is not enough: the nonce is 16
//! bits, so recall that asked about every handle-SHAPED string let one tool
//! result carry thousands of guesses (Reviewer E, round 18). Recall asks only
//! about the handles a turn RECORDS holding when it was sealed
//! (`Provenance::handles`); a planted guess was never held, so it is never
//! recorded and never asked. A log older than that field falls back to the
//! turn's own text — never a tool's output — capped.
//!
//! # One store: Shannon's own
//!
//! Steven, 10-23: a mark is kept and a reopened conversation shows the value
//! to the user; the one choice, `[secrets] password_protect`, is whether
//! opening it asks for a password first — off by default. Every kept secret
//! lives in [`FileVault`], Shannon's owner-only files, whatever that setting
//! says: a setting never decides WHERE a secret is, so no change of it can
//! leave one where the next run does not look (`069099a` kept them in the OS
//! store when protected, and a session-only `:set` moved them there for
//! good; `docs/Reviews/2026-10-23-password-protect/`).
//!
//! What stands between the plain files and another program running as the
//! user — the agent's tools included — is the permission gate (the state
//! directory is a protected path) and the model never being told where they
//! are. No store that asks nothing can do better. Protection is sealing a
//! record under a passphrase Shannon asks for — not built yet (the plan's
//! P2–P5, `docs/Milestones/TODO.md`); until it is, the setting changes
//! nothing and says so.
//!
//! The OS's secure store is not used: it asks "is this the same program?",
//! not "is the user here?" — never the binary that made an item, always an
//! unsigned rebuild.
//!
//! [`NoVault`] where a store cannot be had (no state directory): marking
//! still works, the mark just ends with the run, and says so.
//!
//! # Not kept (v1)
//!
//! Secrets lent from the environment or the config get a FRESH handle every
//! launch. Keeping each would grow the store by one item per launch per
//! variable, forever; they are lent again instead (the model is told the
//! current handles every turn), and an old one in a resumed history refuses
//! cleanly. Keeping them needs the run's seed kept instead — docketed.

use crate::secret::Secret;

/// What a vault keeps for one handle: enough to put the secret back under it.
#[derive(Clone, Debug)]
pub struct Kept {
    /// The name it was minted under (`secret`, `github-token`).
    pub name: String,
    /// The value. A [`Secret`], so `Debug` cannot print it.
    pub value: Secret,
    /// Whether a command may expand it (`:secret --no-spend` says not).
    pub spendable: bool,
}

/// The stored form's version, so a later format can still read this one.
const FORMAT: u64 = 1;

impl Kept {
    /// The string a backend stores. Versioned JSON; the value is inside, so
    /// only a backend calls this, and only to hand it to its secure store.
    pub fn encode(&self) -> String {
        serde_json::json!({
            "v": FORMAT,
            "name": self.name,
            "value": self.value.expose(),
            "spendable": self.spendable,
        })
        .to_string()
    }

    /// Read back what [`encode`](Self::encode) wrote. The reason for a refusal
    /// never quotes the stored text, which holds a value.
    pub fn decode(stored: &str) -> Result<Kept, VaultError> {
        let corrupt = |why: &str| VaultError::Corrupt(why.to_string());
        let v: serde_json::Value =
            serde_json::from_str(stored).map_err(|_| corrupt("not the format Shannon writes"))?;
        if v.get("v").and_then(serde_json::Value::as_u64) != Some(FORMAT) {
            return Err(corrupt("written by a different version of Shannon"));
        }
        let text = |key: &str| {
            v.get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| corrupt("a field is missing"))
        };
        Ok(Kept {
            name: text("name")?,
            value: Secret::new(text("value")?),
            spendable: v
                .get("spendable")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| corrupt("a field is missing"))?,
        })
    }
}

/// Why a vault could not keep or recall.
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    /// No store can be had (no state directory): marks end with the run.
    #[error("there is no store for it, so it ends with this run")]
    Unsupported,
    /// The store refused or failed — a permission, a full disk, a directory
    /// someone else could reach.
    #[error("the store refused: {0}")]
    Store(String),
    /// What was kept could not be read back.
    #[error("a kept secret could not be read: {0}")]
    Corrupt(String),
}

/// A store for secrets between runs — [`FileVault`] in production, a trait
/// so a test keeps them in memory. See the module doc.
pub trait Vault: Send + Sync {
    /// What this vault is, for messages: "Shannon's own store".
    fn name(&self) -> &str;
    /// Keep `kept` under `handle`, replacing what was there.
    fn keep(&self, handle: &str, kept: &Kept) -> Result<(), VaultError>;
    /// What was kept under exactly `handle`, or `None`.
    fn recall(&self, handle: &str) -> Result<Option<Kept>, VaultError>;
    /// Drop what was kept under `handle`; nothing kept is not an error.
    fn forget(&self, handle: &str) -> Result<(), VaultError>;
    /// Every handle kept here — for the passphrase's seal-all and unseal-all
    /// (the plan's P5, its one coming reader).
    fn handles(&self) -> Result<Vec<String>, VaultError>;
}

/// Where there is no store (no state directory): nothing is kept, and
/// keeping says so.
pub struct NoVault;

impl Vault for NoVault {
    fn name(&self) -> &str {
        "no store"
    }
    fn keep(&self, _: &str, _: &Kept) -> Result<(), VaultError> {
        Err(VaultError::Unsupported)
    }
    fn recall(&self, _: &str) -> Result<Option<Kept>, VaultError> {
        Ok(None)
    }
    fn forget(&self, _: &str) -> Result<(), VaultError> {
        Ok(())
    }
    fn handles(&self) -> Result<Vec<String>, VaultError> {
        Ok(Vec::new())
    }
}

/// **Shannon's own store, which asks nothing** — every kept secret, one
/// owner-only file per handle in its directory (`Config::kept_secrets_dir`,
/// `$XDG_STATE_HOME/shannon/secrets/`), created only when something is
/// first kept.
///
/// **What protects it is the gate, not the store** (the module doc): the
/// directory is under the state directory every gate rule protects, and
/// the model is never told it. A plaintext store of secrets, so every
/// property that keeps it private is enforced here rather than assumed, as
/// `keep_unredacted` does for its record:
/// - the directory is created `0700`, and refused if it is not a plain
///   directory or is open to anyone else — a symlink planted at its name is
///   not followed, and a loosened one is not written into;
/// - a record is written to a fresh `0600` sibling (`create_new`, so
///   nothing at that name is overwritten or written through) and renamed
///   over the old, so a reader never meets half a record;
/// - a record is read only if it is a plain file (`O_NOFOLLOW`), and only
///   up to [`RECORD_CAP`] bytes.
///
/// A record's name is its handle in hex, so any handle names exactly one
/// file and no handle names a path.
pub struct FileVault {
    dir: std::path::PathBuf,
}

/// The most a kept record is read to: a value plus its name, far past any
/// real one — and a bound on what a planted file can make Shannon read.
pub const RECORD_CAP: u64 = 64 * 1024;

impl FileVault {
    /// The store in `dir`, touched on the first keep.
    pub fn new(dir: impl Into<std::path::PathBuf>) -> Self {
        FileVault { dir: dir.into() }
    }

    fn path(&self, handle: &str) -> std::path::PathBuf {
        let hex: String = handle.bytes().map(|b| format!("{b:02x}")).collect();
        self.dir.join(format!("{hex}.json"))
    }

    /// The handle a record's file name stands for, if it is one of ours.
    fn handle_of(file: &str) -> Option<String> {
        let hex = file.strip_suffix(".json")?;
        if hex.is_empty() || hex.len() % 2 != 0 {
            return None;
        }
        let bytes: Option<Vec<u8>> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
            .collect();
        String::from_utf8(bytes?).ok()
    }

    /// The directory, made if it is not there — and refused unless it is a
    /// plain directory only its owner can enter.
    fn ensure_dir(&self) -> Result<(), VaultError> {
        let failed = |e: std::io::Error| VaultError::Store(format!("{}: {e}", self.dir.display()));
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(&self.dir).map_err(failed)?;
        let meta = std::fs::symlink_metadata(&self.dir).map_err(failed)?;
        if !meta.is_dir() {
            return Err(VaultError::Store(format!(
                "{} is not a plain directory",
                self.dir.display()
            )));
        }
        #[cfg(unix)]
        if std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o077 != 0 {
            return Err(VaultError::Store(format!(
                "{} is open to other users — `chmod 700` it",
                self.dir.display()
            )));
        }
        Ok(())
    }
}

impl Vault for FileVault {
    fn name(&self) -> &str {
        "Shannon's own store"
    }
    fn keep(&self, handle: &str, kept: &Kept) -> Result<(), VaultError> {
        use std::io::Write as _;
        self.ensure_dir()?;
        let path = self.path(handle);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let fresh = self
            .dir
            .join(format!(".{}.{nanos}.tmp", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let written = options.open(&fresh).and_then(|mut file| {
            file.write_all(kept.encode().as_bytes())?;
            file.sync_all()
        });
        let renamed = written.and_then(|()| std::fs::rename(&fresh, &path));
        renamed.map_err(|e| {
            let _ = std::fs::remove_file(&fresh);
            VaultError::Store(format!("{}: {e}", self.dir.display()))
        })
    }
    fn recall(&self, handle: &str) -> Result<Option<Kept>, VaultError> {
        use std::io::Read as _;
        let path = self.path(handle);
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NOFOLLOW);
        let file = match options.open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(VaultError::Store(format!("{}: {e}", path.display()))),
        };
        let plain = file.metadata().is_ok_and(|m| m.is_file());
        if !plain {
            return Err(VaultError::Corrupt("not a plain file".into()));
        }
        let mut stored = String::new();
        file.take(RECORD_CAP + 1)
            .read_to_string(&mut stored)
            .map_err(|_| VaultError::Corrupt("not text".into()))?;
        if stored.len() as u64 > RECORD_CAP {
            return Err(VaultError::Corrupt(
                "larger than any record Shannon writes".into(),
            ));
        }
        Kept::decode(&stored).map(Some)
    }
    fn forget(&self, handle: &str) -> Result<(), VaultError> {
        match std::fs::remove_file(self.path(handle)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(VaultError::Store(format!("{}: {e}", self.dir.display())))
            }
            _ => Ok(()),
        }
    }
    fn handles(&self) -> Result<Vec<String>, VaultError> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(VaultError::Store(format!("{}: {e}", self.dir.display()))),
        };
        let mut handles: Vec<String> = entries
            .filter_map(|entry| Self::handle_of(entry.ok()?.file_name().to_str()?))
            .collect();
        handles.sort();
        Ok(handles)
    }
}

/// A vault in memory, for tests. Stores the ENCODED form, so every round
/// trip through it exercises [`Kept::encode`] and [`Kept::decode`] as a real
/// backend does. Shared across the "runs" of a test by `Arc`.
#[cfg(any(test, feature = "test-util"))]
#[derive(Default)]
pub struct MemoryVault(std::sync::Mutex<std::collections::HashMap<String, String>>);

#[cfg(any(test, feature = "test-util"))]
impl MemoryVault {
    /// How many handles are kept — for a test's premise.
    pub fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Whether nothing is kept.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(any(test, feature = "test-util"))]
impl Vault for MemoryVault {
    fn name(&self) -> &str {
        "memory"
    }
    fn keep(&self, handle: &str, kept: &Kept) -> Result<(), VaultError> {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(handle.to_string(), kept.encode());
        Ok(())
    }
    fn recall(&self, handle: &str) -> Result<Option<Kept>, VaultError> {
        let map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.get(handle).map(|s| Kept::decode(s)).transpose()
    }
    fn forget(&self, handle: &str) -> Result<(), VaultError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(handle);
        Ok(())
    }
    fn handles(&self) -> Result<Vec<String>, VaultError> {
        let map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut handles: Vec<String> = map.keys().cloned().collect();
        handles.sort();
        Ok(handles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What is stored reads back as what was kept, and a refusal to read
    /// never quotes the stored text — it holds a value.
    #[test]
    fn kept_round_trips_and_a_bad_record_does_not_echo_itself() {
        let kept = Kept {
            name: "secret".into(),
            value: Secret::new("the eagle lands at noon, café #3!"),
            spendable: false,
        };
        let back = Kept::decode(&kept.encode()).expect("round trip");
        assert_eq!(back.name, "secret");
        assert_eq!(back.value.expose(), "the eagle lands at noon, café #3!");
        assert!(!back.spendable);

        let bad = r#"{"v":1,"name":"x","value":"hunter2"}"#;
        let why = Kept::decode(bad)
            .expect_err("spendable is missing")
            .to_string();
        assert!(!why.contains("hunter2"), "{why}");
        assert!(
            Kept::decode(r#"{"v":9}"#).is_err(),
            "another version is refused"
        );
    }

    #[test]
    fn no_vault_keeps_nothing_and_says_so() {
        let kept = Kept {
            name: "s".into(),
            value: Secret::new("v"),
            spendable: true,
        };
        assert!(matches!(
            NoVault.keep("{{s.1234}}", &kept),
            Err(VaultError::Unsupported)
        ));
        assert!(NoVault.recall("{{s.1234}}").unwrap().is_none());
    }

    fn kept(value: &str) -> Kept {
        Kept {
            name: "secret".into(),
            value: Secret::new(value),
            spendable: true,
        }
    }

    /// **The plain store round-trips, lists, forgets — and is private**: the
    /// directory `0700`, each record `0600`, made only on the first keep, and
    /// a handle of any shape names one file inside it.
    #[test]
    fn file_vault_keeps_recalls_lists_and_forgets_privately() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("secrets");
        let v = FileVault::new(&dir);
        assert!(v.recall("{{secret.06ea}}").unwrap().is_none());
        assert!(v.handles().unwrap().is_empty());
        assert!(!dir.exists(), "nothing made until something is kept");

        v.keep("{{secret.06ea}}", &kept("hunter2")).unwrap();
        v.keep("{{../../etc/x.0001}}", &kept("s3cr3t")).unwrap();
        let back = v.recall("{{secret.06ea}}").unwrap().expect("kept");
        assert_eq!(back.value.expose(), "hunter2");
        assert_eq!(
            v.handles().unwrap(),
            ["{{../../etc/x.0001}}", "{{secret.06ea}}"]
        );
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            assert!(
                entry.file_name().to_str().unwrap().ends_with(".json"),
                "only records, in the directory: {entry:?}"
            );
            #[cfg(unix)]
            assert_eq!(
                std::os::unix::fs::PermissionsExt::mode(&entry.metadata().unwrap().permissions())
                    & 0o777,
                0o600
            );
        }
        #[cfg(unix)]
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(
                &std::fs::metadata(&dir).unwrap().permissions()
            ) & 0o777,
            0o700
        );

        v.keep("{{secret.06ea}}", &kept("hunter3")).unwrap();
        let back = v.recall("{{secret.06ea}}").unwrap().expect("replaced");
        assert_eq!(back.value.expose(), "hunter3");
        v.forget("{{secret.06ea}}").unwrap();
        v.forget("{{secret.06ea}}")
            .expect("nothing kept is not an error");
        assert!(v.recall("{{secret.06ea}}").unwrap().is_none());
        assert_eq!(v.handles().unwrap(), ["{{../../etc/x.0001}}"]);
    }

    /// **A store someone else could reach is refused, never written into**:
    /// a symlink at the directory's name, a directory open to others, a
    /// record that is a link, a record too large to be ours.
    #[cfg(unix)]
    #[test]
    fn file_vault_refuses_what_it_did_not_make_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::TempDir::new().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o700)).unwrap();
        let linked = tmp.path().join("linked");
        std::os::unix::fs::symlink(&elsewhere, &linked).unwrap();
        let why = FileVault::new(&linked)
            .keep("{{s.1}}", &kept("v"))
            .expect_err("a symlink is not followed");
        assert!(why.to_string().contains("not a plain directory"), "{why}");
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);

        let open = tmp.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        let why = FileVault::new(&open)
            .keep("{{s.1}}", &kept("v"))
            .expect_err("open to others");
        assert!(why.to_string().contains("chmod 700"), "{why}");
        assert_eq!(std::fs::read_dir(&open).unwrap().count(), 0);

        let dir = tmp.path().join("secrets");
        let v = FileVault::new(&dir);
        v.keep("{{s.1}}", &kept("v")).unwrap();
        let target = tmp.path().join("target.json");
        std::fs::write(&target, kept("planted").encode()).unwrap();
        std::os::unix::fs::symlink(&target, v.path("{{s.2}}")).unwrap();
        assert!(
            v.recall("{{s.2}}").is_err(),
            "a record that is a link is not read"
        );
        std::fs::write(v.path("{{s.3}}"), "x".repeat(RECORD_CAP as usize + 1)).unwrap();
        let why = v.recall("{{s.3}}").expect_err("too large");
        assert!(
            matches!(&why, VaultError::Corrupt(r) if r.contains("larger")),
            "refused for its size, not as text cut short: {why}"
        );
    }
}
