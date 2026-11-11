//! **What a sed script does**, command by command.
//!
//! The script is the whole danger of `sed`: `w FILE` writes, `r FILE`
//! reads a file no argument names, `e` runs the pattern space as a
//! command, and `s///e` does the same; `s///w FILE` writes. Everything
//! else — `p`, `d`, a plain `s`, addresses — reads and prints. This reader
//! strips addresses (`1,5`, `$`, `/re/`, `!`, `~`) and looks at the
//! command letter of each `;`- or newline-separated piece. It does not
//! parse `{ }` blocks or `y`: a block's inner commands are read as pieces
//! of their own, which is enough for the commands that matter.

use super::{DYNAMIC, Found};
use crate::permission::model::Role;

/// The files a sed script reads and writes, and the code it runs.
///
/// Read as sed reads it, one command at a time: addresses first, then the
/// command letter, then what that command takes — `s` and `y` their
/// delimited parts (a `;` inside them is text, not a separator: `s/a;b/c/w
/// FILE` writes — review 2026-10-10, B), `w`/`r`/`e`/`a`/`i`/`c` the rest
/// of the LINE, labels a word, `{` and `}` nothing. A letter this reader
/// does not know is code it cannot read — fail closed.
pub fn scan(script: &str) -> Vec<Found> {
    let chars: Vec<char> = script.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    // The rest of the line from `i`, trimmed; `i` moves past the newline.
    let line_arg = |i: &mut usize| -> String {
        let start = *i;
        while *i < chars.len() && chars[*i] != '\n' {
            *i += 1;
        }
        let s: String = chars[start..*i].iter().collect();
        s.trim().to_string()
    };
    // Past a delimited part starting at `i` (just after the opening
    // delimiter), to the index of its closing delimiter, honouring `\`.
    let delimited_end = |mut k: usize, delim: char| -> Option<usize> {
        while k < chars.len() {
            match chars[k] {
                '\\' => k += 2,
                c if c == delim => return Some(k),
                _ => k += 1,
            }
        }
        None
    };
    while i < chars.len() {
        // Separators, blocks, blanks.
        if matches!(chars[i], ';' | '\n' | ' ' | '\t' | '{' | '}') {
            i += 1;
            continue;
        }
        // Addresses: numbers, `$`, `,`, `~`, `!`, `/re/`, `\cREc`, and the
        // blanks between.
        loop {
            match chars.get(i) {
                Some(c) if c.is_ascii_digit() || matches!(c, '$' | ',' | ' ' | '~' | '!') => i += 1,
                Some('/') => match delimited_end(i + 1, '/') {
                    Some(end) => i = end + 1,
                    // An unterminated address reads no command.
                    None => return out,
                },
                Some('\\') => {
                    let Some(&d) = chars.get(i + 1) else {
                        return out;
                    };
                    match delimited_end(i + 2, d) {
                        Some(end) => i = end + 1,
                        None => return out,
                    }
                }
                _ => break,
            }
        }
        let Some(&c) = chars.get(i) else { break };
        i += 1;
        // `1{`: the address opens a block; its commands follow.
        if c == '{' {
            continue;
        }
        match c {
            's' | 'y' => {
                let Some(&delim) = chars.get(i) else {
                    out.push(Found::code("sed: an `s` with no delimiter"));
                    break;
                };
                // Two delimited parts, then the flags (for `s`) up to a
                // separator; a `w` flag's file is the rest of the LINE.
                let Some(e1) = delimited_end(i + 1, delim) else {
                    break;
                };
                let Some(e2) = delimited_end(e1 + 1, delim) else {
                    break;
                };
                i = e2 + 1;
                if c == 'y' {
                    continue;
                }
                let mut flags = String::new();
                while i < chars.len() && !matches!(chars[i], ';' | '\n' | '}') {
                    if chars[i] == 'w' {
                        i += 1;
                        let file = line_arg(&mut i);
                        out.push(Found::file(Role::WriteFile, file_name(&file)));
                        flags.push('w');
                        break;
                    }
                    flags.push(chars[i]);
                    i += 1;
                }
                if flags.contains('e') {
                    out.push(Found::code(
                        "sed `s///e`: runs the pattern space as a command",
                    ));
                }
            }
            'w' | 'W' => {
                let file = line_arg(&mut i);
                out.push(Found::file(Role::WriteFile, file_name(&file)));
            }
            'r' | 'R' => {
                let file = line_arg(&mut i);
                out.push(Found::file(Role::ReadFile, file_name(&file)));
            }
            'e' => {
                line_arg(&mut i);
                out.push(Found::code(
                    "sed `e`: runs the pattern space, or its argument, as a command",
                ));
            }
            // Text to the end of the line (GNU's one-line form).
            'a' | 'i' | 'c' => {
                line_arg(&mut i);
            }
            // A label, a branch, a comment: a word, or the line.
            ':' | 'b' | 't' | 'T' => {
                while i < chars.len() && !matches!(chars[i], ';' | '\n') {
                    i += 1;
                }
            }
            '#' => {
                line_arg(&mut i);
            }
            // Takes an optional number.
            'q' | 'Q' | 'l' | 'L' => {
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                if let Some(trailing) = trailing(&chars, i) {
                    out.push(Found::code(&format!(
                        "sed: `{trailing}` after `{c}`, which sed reads as more commands"
                    )));
                    break;
                }
            }
            // Takes nothing: more text after it is sed reading further
            // commands, or refusing the script (`1pw`) — either way not
            // the print it looks like.
            'p' | 'P' | 'd' | 'D' | 'n' | 'N' | 'g' | 'G' | 'h' | 'H' | 'x' | '=' | 'z' | 'F' => {
                if let Some(trailing) = trailing(&chars, i) {
                    out.push(Found::code(&format!(
                        "sed: `{trailing}` after `{c}`, which sed reads as more commands"
                    )));
                    break;
                }
            }
            'v' => {
                while i < chars.len() && !matches!(chars[i], ';' | '\n' | '}') {
                    i += 1;
                }
            }
            other => {
                out.push(Found::code(&format!(
                    "sed: `{other}` is a command this reader does not know"
                )));
                break;
            }
        }
    }
    out
}

/// Text after a command that takes none, before the next separator or a
/// closing brace: what sed reads as further commands.
fn trailing(chars: &[char], from: usize) -> Option<String> {
    let mut k = from;
    while k < chars.len() && matches!(chars[k], ' ' | '\t') {
        k += 1;
    }
    if k >= chars.len() || matches!(chars[k], ';' | '\n' | '}') {
        return None;
    }
    let end = chars[k..]
        .iter()
        .position(|c| matches!(c, ';' | '\n'))
        .map_or(chars.len(), |p| k + p);
    Some(chars[k..end].iter().collect())
}

/// The file a `w`/`r` names; sed takes the rest of the line, so an empty
/// name is "whatever follows", which nothing can place.
fn file_name(arg: &str) -> String {
    if arg.is_empty() {
        DYNAMIC.to_string()
    } else {
        arg.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(script: &str) -> Vec<(Role, String)> {
        scan(script).into_iter().map(|f| (f.role, f.what)).collect()
    }

    /// **A script that prints reads nothing it was not given.**
    #[test]
    fn printing_and_substituting_find_nothing() {
        for s in [
            "1,60p",
            "$p",
            "10,$p",
            "s/foo/bar/",
            "s/a/b/g",
            "/x/d",
            "p;p",
            "1!d",
        ] {
            assert!(scan(s).is_empty(), "{s}: {:?}", scan(s));
        }
    }

    /// **`w` and `s///w` write the named file; `r` reads one.**
    #[test]
    fn writes_and_reads_are_found_with_their_files() {
        assert_eq!(roles("1w out.txt"), [(Role::WriteFile, "out.txt".into())]);
        assert_eq!(
            roles("s/a/b/w /tmp/x"),
            [(Role::WriteFile, "/tmp/x".into())]
        );
        assert_eq!(
            roles("/x/r /etc/hosts"),
            [(Role::ReadFile, "/etc/hosts".into())]
        );
        assert_eq!(roles("1p;w log"), [(Role::WriteFile, "log".into())]);
        assert_eq!(roles("1w"), [(Role::WriteFile, DYNAMIC.into())]);
    }

    /// **`e` and `s///e` run the pattern space** — code, not a file.
    #[test]
    fn e_runs_a_command() {
        assert_eq!(scan("1e").len(), 1);
        assert_eq!(scan("1e")[0].role, Role::Code);
        assert_eq!(scan("s/^/echo /e")[0].role, Role::Code);
        // A delimiter that is not `/`, and an escaped delimiter inside.
        assert_eq!(scan("s|a\\|b|c|e")[0].role, Role::Code);
    }

    /// **An address with a slash in its regex does not hide the command.**
    #[test]
    fn a_regex_address_is_stepped_over() {
        assert_eq!(roles("/a\\/b/w f"), [(Role::WriteFile, "f".into())]);
        assert!(
            scan("/unterminated w f").is_empty(),
            "an unterminated address reads no command"
        );
    }

    /// **A `;` inside `s`'s parts is text, a `{` is no address, and a
    /// letter this reader does not know is code** (review 2026-10-10, B:
    /// each of these read as a plain print and ran unasked).
    #[test]
    fn separators_inside_a_substitution_hide_nothing() {
        assert_eq!(
            roles("s/a;b/c/w /tmp/out"),
            [(Role::WriteFile, "/tmp/out".into())]
        );
        assert_eq!(
            roles("s/a/b;c/w /tmp/out"),
            [(Role::WriteFile, "/tmp/out".into())]
        );
        assert_eq!(scan("s/x/date;true/e")[0].role, Role::Code);
        assert_eq!(
            roles("1{w /tmp/out\n}"),
            [(Role::WriteFile, "/tmp/out".into())]
        );
        assert_eq!(scan("1{e date\n}")[0].role, Role::Code);
        // The file is the rest of the LINE, `;` included.
        assert_eq!(roles("w a;b"), [(Role::WriteFile, "a;b".into())]);
        // `y` is two parts and no flag; a label is a word.
        assert!(scan("y/abc/xyz/;:top;b top").is_empty());
        // Unknown: code, not a read.
        assert_eq!(scan("1k")[0].role, Role::Code);
    }
}
