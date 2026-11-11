//! System-clipboard bridge via OSC 52.
//!
//! A yank mirrors its text to the host clipboard by emitting the OSC 52
//! escape (`ESC ] 52 ; c ; <base64> BEL`) — the terminal copies the
//! decoded bytes into its clipboard. This beats linking a platform
//! clipboard library on three counts: it needs no X11/Wayland/daemon, it
//! works **over SSH** (the escape rides the same PTY as everything else),
//! and it's one `write!` rather than a per-OS backend.
//!
//! The seam is a trait ([`OscSink`]) so the app depends on "copy this text"
//! rather than on stdout — the editor stays testable (an in-memory sink in
//! tests) and a future non-TUI frontend can copy however it likes.
//!
//! Scope notes:
//! - **Outbound only.** OSC 52 *reads* (querying the clipboard back) are
//!   widely disabled for security and need a terminal reply event; `p`
//!   pastes the internal register and bracketed paste brings outside text
//!   *in*, so we never read.
//! - **tmux** swallows a plain OSC 52 — it interprets escape sequences and only
//!   forwards one to the outer terminal with `set-clipboard on`. So when `$TMUX`
//!   is set we wrap the sequence in tmux's **DCS passthrough** (`ESC Ptmux ; …
//!   ESC \`, inner ESCs doubled), which reaches the outer terminal directly
//!   regardless of `set-clipboard`. tmux must permit it: `set -g
//!   allow-passthrough on` (3.3+). Verified by hand: the plain sequence works
//!   direct and over SSH but is swallowed by tmux; the wrap fixes tmux.
//!
//! What a copy took is a [`Copied`], not a string: the rows it came from and
//! the span of each, so a secret it touches is found by POSITION — a drag
//! across half a key touches the key, which no test on the copied text alone
//! can see.

use std::io::{self, Write};

use base64::Engine;
use shannon_core::Aliases;

/// **What a copy took, with the context to judge it** — each row it covers
/// whole, and the span taken from that row. The text is a rendering of it
/// ([`text`](Self::text)); so is the text with every secret it touches as its
/// handle ([`with_handles`](Self::with_handles)). The buffer's `read` is
/// `text()`, so a yank, a cut and a drag can never disagree about what they
/// took.
///
/// A secret is matched within one row, as the screen highlights it
/// (`view::row_secrets`): a value holding a newline is never matched — the
/// same limit, in the same place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copied {
    pieces: Vec<Piece>,
    /// Linewise: every row ends with `'\n'`, the last included. Otherwise
    /// rows join with `'\n'` and none trails.
    linewise: bool,
}

/// One row's share of a copy: the row, and the byte span `[lo, hi)` taken.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Piece {
    row: String,
    lo: usize,
    hi: usize,
}

impl Copied {
    /// A copy of `rows`, each with the CHAR span `[lo, hi)` taken from it
    /// (clamped to the row); `linewise` as a linewise register ends rows.
    pub fn of<'a>(
        rows: impl IntoIterator<Item = (&'a [char], usize, usize)>,
        linewise: bool,
    ) -> Copied {
        let byte = |row: &[char], col: usize| -> usize {
            row[..col.min(row.len())].iter().map(|c| c.len_utf8()).sum()
        };
        let pieces = rows
            .into_iter()
            .map(|(row, lo, hi)| {
                let hi = hi.min(row.len());
                Piece {
                    row: row.iter().collect(),
                    lo: byte(row, lo.min(hi)),
                    hi: byte(row, hi),
                }
            })
            .collect();
        Copied { pieces, linewise }
    }

    /// The text taken.
    pub fn text(&self) -> String {
        self.render(|p| p.row[p.lo..p.hi].to_string())
    }

    /// The handles of the secrets this copy touches — whole or in part, each
    /// once, in order of first appearance.
    pub fn secrets_touched(&self, aliases: &Aliases) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for piece in &self.pieces {
            for (_, _, handle) in touched(piece, aliases) {
                if !out.iter().any(|o| o == handle) {
                    out.push(handle.to_string());
                }
            }
        }
        out
    }

    /// **Each held value this copy took only part of** — the part, and the
    /// handle of the value it is part of — each once, in order. A part is no
    /// literal match for its value, so pasted and sent it left in clear,
    /// unstyled, with nothing said (review stop 14, E F4); the app marks it
    /// as a secret of its own (`App::mark_parts_taken`). A value taken whole
    /// is not here: it is masked as itself.
    pub fn parts_taken<'a>(&self, aliases: &'a Aliases) -> Vec<(String, &'a str)> {
        let mut out: Vec<(String, &'a str)> = Vec::new();
        for piece in &self.pieces {
            for (start, end, handle) in touched(piece, aliases) {
                if piece.lo <= start && end <= piece.hi {
                    continue;
                }
                let part = piece.row[start.max(piece.lo)..end.min(piece.hi)].to_string();
                if !out.iter().any(|(p, h)| *p == part && *h == handle) {
                    out.push((part, handle));
                }
            }
        }
        out
    }

    /// The text taken, with every secret it touches — any part of it — as
    /// that secret's handle, once per occurrence.
    pub fn with_handles(&self, aliases: &Aliases) -> String {
        self.render(|p| {
            let mut out = String::new();
            let mut at = p.lo;
            // How far the handles emitted so far reach — UNclamped, unlike `at`:
            // clamped to the span, an inner secret ending past the span looked
            // uncovered and emitted its handle beside the outer one (Reviewer
            // A, round 19).
            let mut covered = p.lo;
            for (start, end, handle) in touched(p, aliases) {
                // Wholly inside what an earlier handle already stands for.
                if end <= covered {
                    continue;
                }
                out.push_str(&p.row[at..start.max(at).min(p.hi)]);
                out.push_str(handle);
                covered = end;
                at = end.min(p.hi);
            }
            out.push_str(&p.row[at..p.hi]);
            out
        })
    }

    /// The one join rule: each piece rendered, then joined as the shape
    /// requires.
    fn render(&self, piece: impl FnMut(&Piece) -> String) -> String {
        let parts = self.pieces.iter().map(piece);
        if self.linewise {
            parts.map(|p| p + "\n").collect()
        } else {
            parts.collect::<Vec<_>>().join("\n")
        }
    }
}

/// Every secret occurrence in `piece`'s row that overlaps the span it took, as
/// `(start, end, handle)`, by start and then longest first. Occurrences that
/// overlap EACH OTHER are all kept: dropping one, as a replacement must, would
/// miss a selection that touches only its tail.
fn touched<'a>(piece: &Piece, aliases: &'a Aliases) -> Vec<(usize, usize, &'a str)> {
    if piece.lo >= piece.hi {
        return Vec::new();
    }
    let mut all: Vec<(usize, usize, &'a str)> = aliases
        .secrets()
        .flat_map(|a| {
            let value = a.shown_to_user();
            starts_of(&piece.row, value)
                .into_iter()
                .map(move |i| (i, i + value.len(), a.handle.as_str()))
        })
        .collect();
    all.retain(|&(start, end, _)| start < piece.hi && end > piece.lo);
    all.sort_by_key(|&(start, end, _)| (start, std::cmp::Reverse(end)));
    all
}

/// Every byte offset `needle` starts at in `hay` — overlapping ones too
/// (`aa` in `aaa` is two), which `match_indices` skips.
fn starts_of(hay: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out; // matches everywhere; and would step past the end
    }
    let mut from = 0;
    while let Some(i) = hay[from..].find(needle) {
        let at = from + i;
        out.push(at);
        // One char on, so the next search starts on a char boundary.
        from = at + hay[at..].chars().next().map_or(1, char::len_utf8);
    }
    out
}

/// Something that can put text on the system clipboard. One method, so it's
/// trivially object-safe; the app holds a `Box<dyn OscSink>`.
pub trait OscSink {
    /// Copy `text` to the system clipboard. Best-effort: a terminal that
    /// doesn't understand OSC 52 simply ignores the escape, so failures are
    /// silent by design (a clipboard hiccup must never crash the editor).
    fn set_clipboard(&mut self, text: &str);
}

/// The real sink: emit the OSC 52 escape to stdout. Stateless. Writing a
/// *complete* control sequence to stdout between frames doesn't disturb
/// ratatui's screen buffer — the terminal consumes it as a clipboard op,
/// not as cells.
pub struct TerminalClipboard;

impl OscSink for TerminalClipboard {
    fn set_clipboard(&mut self, text: &str) {
        // **Never from a test**: the escape goes to the real stdout, past
        // libtest's capture, and the developer's terminal obeys it — it set
        // the system clipboard, and once read as a mutant's verdict (d493060).
        // Every test app installs a `MemoryClipboard` (`app/tests.rs`
        // `sandboxed`); refusing loudly here is what holds that line
        // (review, Reviewers B and C: deleting it passed every test).
        if cfg!(test) {
            panic!("a test reached the real clipboard: install a `MemoryClipboard` (`sandboxed`)");
        }
        // Wrap for tmux iff we're inside it (the `$TMUX` env var, read per yank
        // — yanks are human-paced, so the cost is irrelevant).
        let seq = osc52_sequence(text, std::env::var_os("TMUX").is_some());
        let mut out = io::stdout();
        // Best-effort: ignore write/flush errors (see the trait doc).
        let _ = out.write_all(seq.as_bytes());
        let _ = out.flush();
    }
}

/// Build the OSC 52 clipboard escape for `text`. The base form is
/// `ESC ] 52 ; c ; <base64> BEL` (`c` = clipboard vs. `p` primary; BEL the
/// broadly-accepted terminator). Inside tmux (`in_tmux`) it is wrapped in
/// tmux's DCS passthrough — `ESC Ptmux ; <base form, every ESC doubled> ESC \`
/// — so it reaches the outer terminal instead of being swallowed (tmux needs
/// `allow-passthrough on`). Pure + total, so it's unit-tested without stdout.
fn osc52_sequence(text: &str, in_tmux: bool) -> String {
    let payload = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let osc = format!("\x1b]52;c;{payload}\x07");
    if in_tmux {
        // Double the inner ESCs; tmux forwards the unescaped bytes verbatim.
        let inner = osc.replace('\x1b', "\x1b\x1b");
        format!("\x1bPtmux;{inner}\x1b\\")
    } else {
        osc
    }
}

/// In-memory sink for tests across the crate (the app's clipboard-sync
/// tests inject one and inspect what was copied). Shares its log behind an
/// `Arc<Mutex>` so a clone handed to `App` can still be read afterward.
#[cfg(test)]
#[derive(Clone, Default)]
pub struct MemoryClipboard {
    pub log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

#[cfg(test)]
impl MemoryClipboard {
    /// The most recently copied text, if any.
    pub fn last(&self) -> Option<String> {
        self.log.lock().unwrap().last().cloned()
    }

    /// How many times the clipboard was set.
    pub fn count(&self) -> usize {
        self.log.lock().unwrap().len()
    }
}

#[cfg(test)]
impl OscSink for MemoryClipboard {
    fn set_clipboard(&mut self, text: &str) {
        self.log.lock().unwrap().push(text.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table holding one secret per `(name, value)`, and their handles.
    fn held(secrets: &[(&str, &str)]) -> (Aliases, Vec<String>) {
        let mut table = Aliases::default();
        let handles = secrets
            .iter()
            .map(|(name, value)| table.insert(name, value, "test", true).unwrap())
            .collect();
        (table, handles)
    }

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    /// One row, the char span `[lo, hi)` of it.
    fn one(row: &[char], lo: usize, hi: usize) -> Copied {
        Copied::of([(row, lo, hi)], false)
    }

    /// **A secret is touched by POSITION** — any overlap with the span taken,
    /// from either side; a span that only abuts it is not. The text alone could
    /// not tell: `pw=hunt` holds no secret, and is half of one.
    #[test]
    fn a_copy_touches_a_secret_by_position() {
        let (t, h) = held(&[("pw", "hunter2")]);
        let row = chars("pw=hunter2!");
        let touched = |lo, hi| one(&row, lo, hi).secrets_touched(&t);
        assert_eq!(touched(0, 7), h, "the head of it: `pw=hunt`");
        assert_eq!(touched(8, 11), h, "the tail of it: `r2!`");
        assert_eq!(touched(4, 6), h, "inside it");
        assert_eq!(touched(0, 11), h, "all of it");
        assert!(touched(0, 3).is_empty(), "`pw=` abuts it");
        assert!(touched(10, 11).is_empty(), "`!` abuts it");
        assert!(touched(5, 5).is_empty(), "an empty span takes nothing");
    }

    /// **A part of a held value taken is said, with the value it is part of**
    /// (review stop 14, E F4) — the head, the tail, the inside; a value taken
    /// whole is no part, nor one only abutted.
    #[test]
    fn a_part_taken_is_found_with_its_value() {
        let (t, h) = held(&[("pw", "hunter2")]);
        let row = chars("pw=hunter2!");
        let parts = |lo, hi| one(&row, lo, hi).parts_taken(&t);
        let part = |text: &str| vec![(text.to_string(), h[0].as_str())];
        assert_eq!(parts(0, 7), part("hunt"), "the head");
        assert_eq!(parts(8, 11), part("r2"), "the tail");
        assert_eq!(parts(4, 6), part("un"), "inside");
        assert!(parts(3, 10).is_empty(), "the whole value is no part");
        assert!(parts(0, 11).is_empty(), "nor taken with more around it");
        assert!(parts(0, 3).is_empty(), "abutted, nothing of it");
    }

    /// **`with_handles` puts the handle wherever any part of a secret was
    /// taken** — once per occurrence — and leaves the rest as taken.
    #[test]
    fn with_handles_replaces_every_touched_part() {
        let (t, h) = held(&[("pw", "hunter2")]);
        let row = chars("pw=hunter2 and hunter2!");
        assert_eq!(one(&row, 0, 7).with_handles(&t), format!("pw={}", h[0]));
        assert_eq!(one(&row, 8, 15).with_handles(&t), format!("{} and ", h[0]));
        assert_eq!(
            one(&row, 0, 23).with_handles(&t),
            format!("pw={0} and {0}!", h[0])
        );
        assert_eq!(one(&row, 0, 3).with_handles(&t), "pw=", "nothing touched");
    }

    /// Two held values that overlap in the text are BOTH found: a span that
    /// takes only the second one's tail touches it. Keeping one occurrence per
    /// region, as a replacement must, missed exactly that.
    #[test]
    fn overlapping_secrets_are_each_found() {
        let (t, h) = held(&[("a", "abcdef"), ("b", "defgh")]);
        let row = chars("xabcdefghx");
        assert_eq!(
            one(&row, 7, 9).secrets_touched(&t),
            vec![h[1].clone()],
            "`gh`"
        );
        assert_eq!(one(&row, 0, 10).secrets_touched(&t), h);
        assert_eq!(
            one(&row, 0, 10).with_handles(&t),
            format!("x{}{}x", h[0], h[1]),
            "no part of either survives"
        );

        // One held value inside another: the outer handle stands for both,
        // and nothing after it is emitted twice — stepping back to the inner
        // one's end re-emitted the outer value's tail.
        let (t, h) = held(&[("outer", "hunter2!"), ("inner", "hunter2")]);
        let row = chars("pw=hunter2! ok");
        assert_eq!(one(&row, 0, 14).with_handles(&t), format!("pw={} ok", h[0]));
        assert_eq!(one(&row, 0, 14).secrets_touched(&t), h, "both touched");
        // …and when the copy stops inside both, still one handle: the inner
        // one ends past the span, but not past the outer one.
        assert_eq!(one(&row, 0, 9).with_handles(&t), format!("pw={}", h[0]));
    }

    /// A value that overlaps itself (`aa` in `aaa`) is found at every start,
    /// so taking the last char of the second occurrence touches it.
    #[test]
    fn a_self_overlapping_value_is_found_at_every_start() {
        assert_eq!(starts_of("aaa", "aa"), vec![0, 1]);
        assert_eq!(starts_of("éaé", "é"), vec![0, 3], "on char boundaries");
        assert!(
            starts_of("abc", "").is_empty(),
            "an empty needle finds nothing"
        );
        let (t, h) = held(&[("s", "aa")]);
        assert_eq!(one(&chars("xaaa"), 3, 4).secrets_touched(&t), h);
    }

    /// Char spans map to byte offsets — a row of wide and multibyte text
    /// before the value — and a multi-row copy judges each row on its own.
    #[test]
    fn multibyte_rows_and_multi_row_copies() {
        let (t, h) = held(&[("k", "kéy9")]);
        let row = chars("日本 kéy9 ok");
        assert_eq!(one(&row, 0, 5).text(), "日本 ké");
        assert_eq!(one(&row, 0, 5).with_handles(&t), format!("日本 {}", h[0]));

        let first = chars("line one");
        let second = chars("kéy9 here");
        let copied = Copied::of([(&first[..], 5, usize::MAX), (&second[..], 0, 2)], false);
        assert_eq!(copied.text(), "one\nké");
        assert_eq!(copied.secrets_touched(&t), h);
        let lines = Copied::of([(&first[..], 0, usize::MAX)], true);
        assert!(lines.secrets_touched(&t).is_empty());
        assert_eq!(lines.text(), "line one\n", "linewise ends every row");
    }

    #[test]
    fn osc52_plain_sequence_outside_tmux() {
        // Framing pinned: ESC ] 52 ; c ; <base64> BEL.
        assert_eq!(osc52_sequence("hi", false), "\x1b]52;c;aGk=\x07");
    }

    #[test]
    fn osc52_is_dcs_wrapped_inside_tmux() {
        // ESC Ptmux ; <OSC 52, inner ESC doubled> ESC backslash.
        assert_eq!(
            osc52_sequence("hi", true),
            "\x1bPtmux;\x1b\x1b]52;c;aGk=\x07\x1b\\"
        );
    }
}
