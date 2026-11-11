//! **The three over a file's text**, with no file system: what a Read shows,
//! what an Edit makes, and the text both see — so every rule here is a
//! function a test calls with a string.

use super::run::Caps;

/// **A file's text as the tools see it**: UTF-8, every line ended by `\n`
/// whatever the file ends them with, and what [`encode`](Self::encode)
/// needs to write it back as it was.
///
/// A file whose every line ends `\r\n` is read as `\n` and written back
/// `\r\n`: Read never shows a `\r`, so a model's `old_string` never holds
/// one, and every Edit of a Windows file would otherwise miss. A file with
/// a lone `\n` among them is kept exactly, `\r`s and all — normalising
/// would rewrite lines nobody asked to change. A byte-order mark is kept
/// aside, for the same reason Read does not show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    body: String,
    crlf: bool,
    bom: bool,
}

const BOM: char = '\u{feff}';

impl Text {
    /// The file's bytes as text, or why they are not.
    pub fn decode(bytes: &[u8]) -> Result<Text, String> {
        let body = std::str::from_utf8(bytes)
            .map_err(|_| "is not UTF-8 text".to_string())?
            .to_string();
        let (bom, body) = match body.strip_prefix(BOM) {
            Some(rest) => (true, rest.to_string()),
            None => (false, body),
        };
        let crlf = body.contains("\r\n") && !has_lone_lf(&body);
        Ok(Text {
            body: if crlf {
                body.replace("\r\n", "\n")
            } else {
                body
            },
            crlf,
            bom,
        })
    }

    /// The bytes that write this text back as its file wrote it.
    pub fn encode(&self) -> Vec<u8> {
        let body = match self.crlf {
            true => self.body.replace('\n', "\r\n"),
            false => self.body.clone(),
        };
        let mut out = String::with_capacity(body.len() + 3);
        if self.bom {
            out.push(BOM);
        }
        out.push_str(&body);
        out.into_bytes()
    }

    /// `written`, a model's text, in this file's dress — its line endings
    /// and its mark — for a Write over it, or `old`/`new` of an Edit in it.
    pub fn like(&self, written: &str) -> Text {
        Text {
            body: self.matching(written),
            ..self.clone()
        }
    }

    /// `written` as this text's body spells it: a `\r\n` a model sent into a
    /// `\r\n` file is the `\n` its body holds.
    fn matching(&self, written: &str) -> String {
        match self.crlf {
            true => written.replace("\r\n", "\n"),
            false => written.to_string(),
        }
    }

    /// The text, as Read shows it and Edit matches it.
    pub fn body(&self) -> &str {
        &self.body
    }

    /// How many lines it has, as `str::lines` counts them: a last line
    /// with no `\n` is one, an empty file has none.
    pub fn lines(&self) -> usize {
        self.body.lines().count()
    }
}

/// A `\n` not after a `\r`.
fn has_lone_lf(body: &str) -> bool {
    let bytes = body.as_bytes();
    (bytes.iter().enumerate()).any(|(i, &b)| b == b'\n' && (i == 0 || bytes[i - 1] != b'\r'))
}

/// **What a Read shows**: lines `first..=last` of `total`, each `N<tab>line`
/// — as Claude Code's Read results show it, the least numbering `offset`
/// can be aimed by — and how many were cut at the caps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub first: usize,
    pub last: usize,
    pub total: usize,
    pub shown: String,
    /// How many lines were cut, and at how many characters.
    pub long: usize,
    pub cut_at: usize,
}

impl Window {
    /// What the model is told: the lines, and whatever they do not say —
    /// that there is more, where it starts, what was cut. A window that
    /// ends early says how to read on, never just stops.
    pub fn report(&self) -> String {
        if self.total == 0 {
            return "(an empty file)".into();
        }
        if self.first > self.total {
            return format!(
                "(the file has {}; offset {} is past its end)",
                counted(self.total, "line"),
                self.first
            );
        }
        let mut out = self.shown.clone();
        if self.last < self.total {
            out.push_str(&format!(
                "\n[lines {}–{} of {}; read on with offset {}]",
                self.first,
                self.last,
                self.total,
                self.last + 1
            ));
        }
        if self.long > 0 {
            out.push_str(&format!(
                "\n[{} cut at {} characters]",
                counted(self.long, "long line"),
                self.cut_at
            ));
        }
        out
    }
}

/// **The lines a Read of `body` shows** from `offset`, at most `limit` of
/// them, inside `caps`: the window stops at the line that would pass
/// `max_bytes`, so a window is never cut mid-line, and always holds one
/// line when there is one to hold.
///
/// **Each line is `scrub`bed whole before it is cut** — what is numbered
/// and cut is what is sent, so a cut never leaves a secret's prefix
/// (review stop 16, E 1). `body` is a [`Text`]'s: as read, with `scrub`
/// scrubbing each line alone, or already scrubbed with its lines kept
/// (`secrets::redact_lines`) and `scrub` taking each as it is
/// (`files::run::shown` chooses).
pub fn window(
    body: &str,
    offset: usize,
    limit: Option<usize>,
    caps: &Caps,
    mut scrub: impl FnMut(&str) -> String,
) -> Window {
    let total = body.lines().count();
    let first = offset.max(1);
    let most = limit.unwrap_or(usize::MAX).min(caps.max_lines);
    let (mut shown, mut last, mut long) = (String::new(), first.saturating_sub(1), 0);
    for (at, line) in body.lines().enumerate().skip(first - 1).take(most) {
        let line = scrub(line);
        let line = line.as_str();
        let cut = cut_at(line, caps.line_chars);
        let row = format!(
            "{}\t{}{}\n",
            at + 1,
            cut,
            if cut.len() < line.len() { "…" } else { "" }
        );
        if !shown.is_empty() && shown.len() + row.len() > caps.max_bytes {
            break;
        }
        long += usize::from(cut.len() < line.len());
        shown.push_str(&row);
        last = at + 1;
    }
    shown.pop();
    Window {
        first,
        last,
        total,
        shown,
        long,
        cut_at: caps.line_chars,
    }
}

/// `line`'s first `chars` characters, cut on a character's boundary.
fn cut_at(line: &str, chars: usize) -> &str {
    match line.char_indices().nth(chars) {
        Some((at, _)) => &line[..at],
        None => line,
    }
}

/// **Where one replacement fell**: from line `line` of the file before, it
/// replaced `removed` lines with `added` (each the lines its text touches,
/// so at least 1). The record's gutter and read-first's coverage are both
/// read from these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub line: usize,
    pub removed: usize,
    pub added: usize,
}

/// **What an Edit made**: the new text and where each replacement fell, in
/// the order they fell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edited {
    pub text: Text,
    pub places: Vec<Place>,
}

impl Edited {
    /// What the model is told: where its change landed, in the file as it
    /// is now. No snippet — the model wrote the text.
    pub fn report(&self, path: &str) -> String {
        match self.places.as_slice() {
            [one] => {
                let now = lines_named(one.line, one.added);
                match one.added == one.removed {
                    true => format!("Updated {path}: {now}."),
                    false => format!(
                        "Updated {path}: {now} (were {}).",
                        span(one.line, one.removed)
                    ),
                }
            }
            places => {
                let mut shift = 0isize;
                let at: Vec<String> = (places.iter())
                    .take(LINES_LISTED)
                    .map(|place| {
                        let now = (place.line as isize + shift) as usize;
                        shift += place.added as isize - place.removed as isize;
                        now.to_string()
                    })
                    .collect();
                let more = match places.len().saturating_sub(LINES_LISTED) {
                    0 => String::new(),
                    more => format!(", and {more} more"),
                };
                format!(
                    "Updated {path}: {} replacements, at lines {}{more}.",
                    places.len(),
                    at.join(", ")
                )
            }
        }
    }
}

/// How many replacements' lines a report names: every one was 275 KB for
/// 40,000, in the context on every later turn (review stop 16, B 3).
const LINES_LISTED: usize = 20;

/// `line 4` or `lines 4–6`.
fn lines_named(line: usize, count: usize) -> String {
    match count {
        1 => format!("line {line}"),
        _ => format!("lines {}", span(line, count)),
    }
}

/// `4` or `4–6`.
pub(super) fn span(line: usize, count: usize) -> String {
    match count {
        1 => line.to_string(),
        _ => format!("{line}–{}", line + count - 1),
    }
}

/// `1 line` or `2 lines` — a count and its noun agreeing, in every report
/// a file tool makes: a one-line Write said "(1 lines)" (Steven, 11-02).
pub(super) fn counted(n: usize, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        _ => format!("{n} {what}s"),
    }
}

/// **Replace `old` with `new` in `text`**: once, where `old` occurs once, or
/// everywhere when `all`. Refused, as Claude Code words it, where `old`
/// is nowhere, or more than once without `all`, or is empty, or is `new`.
pub fn edit(text: &Text, old: &str, new: &str, all: bool) -> Result<Edited, String> {
    let (old, new) = (text.matching(old), text.matching(new));
    if old.is_empty() {
        return Err("old_string is empty — to make a file, use Write".into());
    }
    if old == new {
        return Err("No changes to make: old_string and new_string are exactly the same.".into());
    }
    let starts: Vec<usize> = text.body.match_indices(&old).map(|(at, _)| at).collect();
    match (starts.len(), all) {
        (0, _) => {
            return Err(format!(
                "String to replace not found in file.\nString: {old}"
            ));
        }
        (1, _) | (_, true) => {}
        (found, false) => {
            return Err(format!(
                "Found {found} matches of the string to replace, but replace_all is false. \
                 To replace all occurrences, set replace_all to true. To replace only one \
                 occurrence, please provide more context to uniquely identify the instance.\n\
                 String: {old}"
            ));
        }
    }
    let touched = |s: &str| s.matches('\n').count() + 1;
    let (removed, added) = (touched(&old), touched(&new));
    // Each match's line counted on from the last, one pass: counted from
    // the start for each, a `replace_all` was matches × file (stop 16, B 3).
    let (mut line, mut counted) = (1, 0);
    let places = (starts.iter())
        .map(|&at| {
            line += text.body[counted..at].matches('\n').count();
            counted = at;
            Place {
                line,
                removed,
                added,
            }
        })
        .collect();
    Ok(Edited {
        text: Text {
            body: text.body.replace(&old, &new),
            ..text.clone()
        },
        places,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn text(s: &str) -> Text {
        Text::decode(s.as_bytes()).unwrap()
    }

    /// **A file is written back as it was read**, every byte — a Windows
    /// file's `\r\n`s, a mark, a mixed file's lone `\r`s — and reads as `\n`
    /// lines only where every line ends `\r\n`.
    #[test]
    fn a_file_reads_as_lines_and_writes_back_as_it_was() {
        let windows = text("a\r\nb\r\n");
        assert_eq!(windows.body(), "a\nb\n");
        assert_eq!(windows.encode(), b"a\r\nb\r\n");
        let mixed = text("a\r\nb\n");
        assert_eq!(
            mixed.body(),
            "a\r\nb\n",
            "a lone \\n keeps the file as it is"
        );
        let marked = text("\u{feff}a\n");
        assert_eq!(marked.body(), "a\n");
        assert_eq!(marked.encode(), "\u{feff}a\n".as_bytes());
        assert_eq!(
            Text::decode(&[0xff, 0xfe]).unwrap_err(),
            "is not UTF-8 text"
        );
        assert_eq!(text("").lines(), 0);
        assert_eq!(text("a").lines(), 1);
        assert_eq!(text("a\nb\n").lines(), 2);
    }

    /// [`window`](super::window), each line taken as it is — for a test
    /// whose subject is the window, not the scrub.
    fn window(body: &str, offset: usize, limit: Option<usize>, caps: &Caps) -> Window {
        super::window(body, offset, limit, caps, str::to_string)
    }

    /// **A line is scrubbed whole, then cut** (review stop 16, E 1): the cut
    /// falls on the scrubbed line, so it never leaves the start of what the
    /// scrub would have taken.
    #[test]
    fn a_line_is_scrubbed_whole_then_cut() {
        let caps = Caps {
            line_chars: 6,
            ..Caps::default()
        };
        let scrub = |line: &str| line.replace("SECRET", "[x]");
        let w = super::window("abcdSECRETxyz\n", 1, None, &caps, scrub);
        assert_eq!(w.shown, "1\tabcd[x…");
    }

    /// **A window numbers its lines and says what it leaves out** — where to
    /// read on, a long line cut on a character, an empty file, an offset past
    /// the end.
    #[test]
    fn a_window_numbers_its_lines_and_says_what_it_leaves() {
        let caps = Caps {
            max_lines: 2,
            line_chars: 3,
            ..Caps::default()
        };
        let five = "one\ntwo\nthree\nfour\nfive\n";
        let w = window(five, 2, None, &caps);
        assert_eq!((w.first, w.last, w.total), (2, 3, 5));
        assert_eq!(
            w.report(),
            "2\ttwo\n3\tthr…\n[lines 2–3 of 5; read on with offset 4]\n\
             [1 long line cut at 3 characters]"
        );
        let all = window(five, 1, Some(9), &Caps::default());
        assert_eq!(all.report(), "1\tone\n2\ttwo\n3\tthree\n4\tfour\n5\tfive");
        // A window from 0 is a window from 1, whoever calls it.
        assert_eq!(window(five, 0, Some(1), &caps).shown, "1\tone");
        assert_eq!(window("", 1, None, &caps).report(), "(an empty file)");
        assert_eq!(
            window(five, 9, None, &caps).report(),
            "(the file has 5 lines; offset 9 is past its end)"
        );
        assert_eq!(
            window("solo\n", 3, None, &caps).report(),
            "(the file has 1 line; offset 3 is past its end)"
        );
        // A character wider than a byte is cut whole.
        let wide = window("ééééé\n", 1, None, &caps);
        assert_eq!(wide.shown, "1\tééé…");
    }

    /// **A window stops at the line that would pass the byte cap** — never
    /// mid-line — and still shows one line taller than the cap.
    #[test]
    fn a_window_stops_before_the_byte_cap() {
        // Each row is 7 bytes with its newline: two fit 14, a third does not.
        let caps = Caps {
            max_bytes: 14,
            ..Caps::default()
        };
        let w = window("aaaa\nbbbb\ncccc\n", 1, None, &caps);
        assert_eq!((w.last, w.shown.as_str()), (2, "1\taaaa\n2\tbbbb"));
        let tall = window(&format!("{}\n", "x".repeat(40)), 1, None, &caps);
        assert_eq!(tall.last, 1, "one line, though it passes the cap");
    }

    /// **An Edit replaces once, or everywhere when asked, and says where** —
    /// refused as Claude Code refuses: not found, found twice, empty, no
    /// change.
    #[test]
    fn an_edit_replaces_once_or_everywhere_and_says_where() {
        let t = text("fn a() {}\nfn b() {}\nfn a2() {}\n");
        let once = edit(&t, "fn b() {}", "fn b() {\n    1\n}", false).unwrap();
        assert_eq!(
            once.text.body(),
            "fn a() {}\nfn b() {\n    1\n}\nfn a2() {}\n"
        );
        assert_eq!(
            once.places,
            [Place {
                line: 2,
                removed: 1,
                added: 3
            }]
        );
        assert_eq!(once.report("a.rs"), "Updated a.rs: lines 2–4 (were 2).");
        let twice = edit(&t, "fn a", "fn z", false).unwrap_err();
        assert!(twice.starts_with("Found 2 matches"), "{twice}");
        let all = edit(&t, "fn a", "fn z", true).unwrap();
        assert_eq!(all.text.body(), "fn z() {}\nfn b() {}\nfn z2() {}\n");
        assert_eq!(
            all.report("a.rs"),
            "Updated a.rs: 2 replacements, at lines 1, 3."
        );
        assert!(
            edit(&t, "fn c", "x", false)
                .unwrap_err()
                .contains("not found")
        );
        assert!(edit(&t, "", "x", false).unwrap_err().contains("use Write"));
        assert!(
            edit(&t, "fn a", "fn a", true)
                .unwrap_err()
                .contains("No changes")
        );
        let same_size = edit(&t, "fn b() {}", "fn c() {}", false).unwrap();
        assert_eq!(same_size.report("a.rs"), "Updated a.rs: line 2.");
    }

    /// **A Windows file is edited as its lines read** — the model's `\n`
    /// and `\r\n` both match, and the file keeps its `\r\n`.
    #[test]
    fn a_windows_file_is_edited_as_it_reads() {
        let t = text("a\r\nb\r\nc\r\n");
        for old in ["a\nb", "a\r\nb"] {
            let done = edit(&t, old, "x\ny", false).unwrap();
            assert_eq!(done.text.encode(), b"x\r\ny\r\nc\r\n", "{old:?}");
        }
    }

    /// **The places of every replacement, shifted by those before it** —
    /// what the report names, in the file as it is now.
    #[test]
    fn later_places_are_named_where_they_are_now() {
        let t = text("x\na\nx\nb\nx\n");
        let done = edit(&t, "x", "y\ny", true).unwrap();
        assert_eq!(
            done.report("f"),
            "Updated f: 3 replacements, at lines 1, 4, 7."
        );
        assert_eq!(done.text.body(), "y\ny\na\ny\ny\nb\ny\ny\n");
        // Two on a line, and one after a line with none: each its own line.
        let done = edit(&text("x x\na\nx\n"), "x", "y", true).unwrap();
        let lines: Vec<usize> = done.places.iter().map(|p| p.line).collect();
        assert_eq!(lines, [1, 1, 3]);
    }

    /// **A report names the first lines and counts the rest** — every one
    /// was 275 KB for 40,000 replacements (review stop 16, B 3).
    #[test]
    fn a_long_report_counts_what_it_does_not_name() {
        let t = text(&"x\na\n".repeat(25));
        let report = edit(&t, "x", "y", true).unwrap().report("f");
        let named: Vec<String> = (0..20).map(|i| (2 * i + 1).to_string()).collect();
        assert_eq!(
            report,
            format!(
                "Updated f: 25 replacements, at lines {}, and 5 more.",
                named.join(", ")
            )
        );
    }

    proptest! {
        /// **Every file that reads writes back byte for byte.**
        #[test]
        fn every_text_round_trips(s in "[a\r\n\u{feff}é]{0,24}") {
            let t = Text::decode(s.as_bytes()).unwrap();
            prop_assert_eq!(t.encode(), s.into_bytes());
        }

        /// **An edit never panics, and changes exactly what it replaced** —
        /// whatever the text and strings: the body grows by `new` less `old`
        /// once per place, and the slicing is by match positions, which are
        /// char boundaries.
        #[test]
        fn an_edit_changes_exactly_what_it_replaced(
            s in "[ab\r\né]{0,24}",
            old in "[ab\r\né]{0,4}",
            new in "[ab\r\né]{0,4}",
            all in any::<bool>(),
        ) {
            let t = Text::decode(s.as_bytes()).unwrap();
            if let Ok(done) = edit(&t, &old, &new, all) {
                let _ = done.report("f");
                let (old, new) = (t.matching(&old), t.matching(&new));
                let places = done.places.len();
                prop_assert_eq!(
                    done.text.body().len() + places * old.len(),
                    t.body().len() + places * new.len()
                );
                prop_assert!(all || places == 1);
            }
        }
    }
}
