//! **A saved time, as the user reads it** (Steven, 10-14: "default to the
//! locale, though what's saved is still the unix time. Local time is just
//! displayed for better human readability").
//!
//! What is saved never changes: a `SystemTime`, the seconds since 1970. This
//! is only how one is shown — `tui.time_format`: `locale` (the C library's
//! `%c`, the locale's own date and time), `unix` (the seconds themselves),
//! or a `strftime` pattern — in the local time zone. The locale is the one
//! the environment names (`LANG`, `LC_TIME`), taken once at launch by
//! [`use_the_locale`]; until then the C library speaks the `C` locale.

use std::ffi::CString;
use std::time::{SystemTime, UNIX_EPOCH};

/// The longest a shown time may be — a pattern that writes more falls back
/// to the seconds (`strftime` says nothing it could not fit).
const MOST_BYTES: usize = 128;

/// **Take the environment's locale for times** — `LC_TIME` alone, so the
/// numbers Shannon parses and prints keep the `C` locale's form. Called once
/// by launch (`main`) before any thread starts: `setlocale` is process-wide
/// and not safe beside a thread reading the locale.
pub fn use_the_locale() {
    // SAFETY: a static NUL-terminated string; called before threads start.
    unsafe {
        libc::setlocale(libc::LC_TIME, c"".as_ptr());
    }
}

/// **`at` as `format` says** (`tui.time_format`): `locale` or empty — the
/// locale's date and time; `unix` — `N s (unix)`; anything else a `strftime`
/// pattern, in local time. A time before 1970 is `—` (it has no seconds to
/// show); one the C library cannot place, or a pattern it cannot write (NUL
/// inside, too long), falls back to the seconds — never to nothing, and
/// never to a made-up date.
pub fn shown(at: SystemTime, format: &str) -> String {
    let Ok(since) = at.duration_since(UNIX_EPOCH) else {
        return "—".to_string();
    };
    let secs = since.as_secs();
    let unix = || format!("{secs}s (unix)");
    let pattern = match format.trim() {
        "unix" => return unix(),
        "locale" | "" => "%c",
        other => other,
    };
    let (Ok(pattern), Ok(t)) = (CString::new(pattern), libc::time_t::try_from(secs)) else {
        return unix();
    };
    // SAFETY: `tm` is plain data, written whole by `localtime_r` before it
    // is read; `strftime` writes at most `buf.len()` bytes and says how many.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return unix();
    }
    let mut buf = [0u8; MOST_BYTES];
    let n = unsafe {
        libc::strftime(
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len(),
            pattern.as_ptr(),
            &tm,
        )
    };
    match n {
        0 => unix(),
        n => String::from_utf8_lossy(&buf[..n]).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// **Shown as asked, and never as nothing** — the seconds for `unix`, a
    /// pattern's fields, the locale's form by default; a pattern that cannot
    /// be written (a NUL, past the bound) gives the seconds back.
    #[test]
    fn a_time_is_shown_as_asked() {
        let at = UNIX_EPOCH + Duration::from_secs(1_760_000_000);
        assert_eq!(shown(at, "unix"), "1760000000s (unix)");
        let year = shown(at, "%Y");
        assert!(year == "2025", "{year}");
        let locale = shown(at, "locale");
        assert!(
            !locale.is_empty() && !locale.ends_with("(unix)") && locale.contains("2025"),
            "{locale}"
        );
        assert_eq!(shown(at, ""), locale, "empty is the locale's");
        assert_eq!(shown(at, "a\0b"), "1760000000s (unix)", "a NUL");
        assert_eq!(
            shown(at, &"%Y".repeat(100)),
            "1760000000s (unix)",
            "past the bound"
        );
        assert_eq!(shown(UNIX_EPOCH - Duration::from_secs(1), "locale"), "—");
        // A time the C library cannot place (`localtime_r` says NULL): the
        // seconds, never a made-up date — `at` is outside a turn's content
        // hash, so a corrupt file can hold one (review of 34033e8, Reviewer B).
        let far = UNIX_EPOCH + Duration::from_secs(i64::MAX as u64);
        assert!(
            shown(far, "locale").ends_with("(unix)"),
            "{}",
            shown(far, "locale")
        );
    }
}
