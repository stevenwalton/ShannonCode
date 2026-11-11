//! **Reading what the model is told** — bounded, as text, never waiting.

use std::io::Read;
use std::path::Path;

/// A file's text, cut at `cap` bytes (on a character) — and whether it was.
pub(crate) struct Text {
    pub text: String,
    pub cut: bool,
}

/// **Read `path` as text, to `cap` bytes.** A regular file only — a device,
/// a pipe or a directory is refused, and opened non-blocking so a FIFO
/// cannot hold the thread that draws the screen (`config::read_file`'s
/// reasons). Invalid UTF-8 is replaced, not refused: an instruction file
/// with one bad byte still says what it says.
pub(crate) fn bounded(path: &Path, cap: usize) -> Result<Text, String> {
    let file = open_regular(path, Links::Follow)?;
    let mut raw = Vec::new();
    file.take(cap as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    let cut = raw.len() > cap;
    raw.truncate(cap);
    let mut text = String::from_utf8_lossy(&raw).into_owned();
    if cut {
        // A cut mid-character leaves a replacement char at the end: drop it.
        while text.ends_with('\u{FFFD}') {
            text.pop();
        }
    }
    Ok(Text { text, cut })
}

/// Whether [`open_regular`] follows a symbolic link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Links {
    Follow,
    Refuse,
}

/// **`path` opened to read, if it is a regular file — never waiting.**
/// Opened non-blocking, so a FIFO put where a file was cannot hold the
/// thread that opens it (the UI's, or a reader's for good), and refused when
/// it is not a regular file: a device, a pipe, a directory. The open every
/// reader of a file it did not make takes — `bounded`, and Claude Code's
/// transcripts (`claude_import`), where a listing that filtered `is_file()`
/// does not stop a swap between the listing and the read (review stop 4,
/// B 6).
///
/// `links`: whether a symbolic link is followed — a user's memory or skill
/// can be one on purpose; a transcript is Claude Code's own file, and one
/// there is another file read as if it were (review stop 4, E 6).
pub(crate) fn open_regular(path: &Path, links: Links) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let refuse = match links {
            Links::Follow => 0,
            Links::Refuse => libc::O_NOFOLLOW,
        };
        options.custom_flags(libc::O_NONBLOCK | refuse);
    }
    #[cfg(not(unix))]
    let _ = links;
    let file = options.open(path).map_err(|e| {
        #[cfg(unix)]
        if links == Links::Refuse && e.raw_os_error() == Some(libc::ELOOP) {
            return "it is a symbolic link, not followed".to_string();
        }
        e.to_string()
    })?;
    let kind = file.metadata().map_err(|e| e.to_string())?;
    if !kind.is_file() {
        return Err("it is not a regular file".into());
    }
    Ok(file)
}

/// **A file's frontmatter and body** — `---` lines around `key: value`
/// lines, as Claude Code's skills and memories write them. Read by hand: a
/// YAML crate for two keys is a dependency without a reason. A value may be
/// quoted, and an indented line continues the one before it (a folded
/// `description: >`). No frontmatter: no keys, the whole text the body. A
/// byte-order mark before it is no text (an editor's, not the author's).
pub(crate) fn frontmatter(text: &str) -> (Vec<(String, String)>, &str) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (Vec::new(), text);
    };
    let mut keys: Vec<(String, String)> = Vec::new();
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        at += line.len();
        let line = line.trim_end_matches(['\n', '\r']);
        if line.trim_end() == "---" {
            return (keys, &rest[at..]);
        }
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = keys.last_mut() {
                let more = line.trim();
                if !value.is_empty() && !more.is_empty() {
                    value.push(' ');
                }
                value.push_str(more);
            }
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim();
            let value = match value {
                ">" | "|" | ">-" | "|-" => "",
                v => v
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                    .unwrap_or(v),
            };
            keys.push((key.trim().to_string(), value.to_string()));
        }
    }
    // Never closed: not frontmatter after all.
    (Vec::new(), text)
}

/// The value of `key` in `keys`, if given and not empty.
pub(crate) fn key<'a>(keys: &'a [(String, String)], key: &str) -> Option<&'a str> {
    keys.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.is_empty())
}
