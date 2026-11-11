//! A string whose contents must never be logged or `Debug`-printed.
//!
//! Wraps a credential (API key today; OAuth tokens in M2) so the
//! no-leak guarantee is *structural* rather than a convention every
//! `info!`/`warn!`/`#[derive(Debug)]` site has to remember. The
//! codebase already enforces invariants with types — `TerminalGuard`
//! for raw-mode restore, `Viewport` clamping at read time — and a
//! credential is exactly the kind of thing that should be impossible
//! to print by accident.
//!
//! The contents are reachable only through `expose()`, which makes
//! every read of the raw value a visible, greppable act. We do *not*
//! pull `secrecy`/`zeroize` for M1: a redacting `Debug` closes the
//! logging vector, which is the realistic leak path here. Zero-on-
//! drop hardening can graduate to the `secrecy` crate when M2 adds
//! more secrets and the dependency earns its place.

/// A credential that redacts itself in `Debug` output.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The one way to read the raw value. Named to make every call
    /// site obvious in review and `grep`.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the value — not even a prefix, which would
        // leak entropy and still be enough to correlate keys.
        f.write_str("Secret(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_does_not_leak_contents() {
        let s = Secret::new("sk-ant-super-secret");
        let rendered = format!("{s:?}");
        assert_eq!(rendered, "Secret(<redacted>)");
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn expose_returns_raw_value() {
        let s = Secret::from("hunter2".to_string());
        assert_eq!(s.expose(), "hunter2");
    }
}
