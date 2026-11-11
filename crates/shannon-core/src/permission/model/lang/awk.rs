//! **What an awk program does** beyond reading its input and printing.
//!
//! `system("…")` and gawk's `@` forms run code; a pipe (`"cmd" | getline`,
//! `print | "cmd"`) runs a command; `> "file"` and `>> "file"` write;
//! `getline < "file"` reads a file no argument names. Each is found by its
//! token, not by a parse of awk: a string holding `system` is a false
//! positive that asks, which is the safe side; a dynamic target (`> f`) is a
//! file nothing can place. `--sandbox` is not a reason to skip any of this:
//! BWK awk ignores the flag (research §7.4).
//!
//! **Code is found by the WORD, not a spelling of the call** (review stop
//! 12, T8; `docs/Security/SECURITY-2026-10-26-awk-runs-a-command-unasked.md`).
//! This looked for the text `system(`, and awk takes white space between a
//! builtin and its parenthesis: `system ("id")` ran any command with no rule,
//! safe unattended. So `system` anywhere, and an `@` before any name —
//! `@load`, `@include`, and `@f()`, gawk's indirect call, which reaches
//! `system` through a string — whatever follows them.

use super::{DYNAMIC, Found};
use crate::permission::model::Role;

/// The files an awk program reads and writes, and the code it runs.
pub fn scan(program: &str) -> Vec<Found> {
    let mut out = Vec::new();
    if has_word(program, "system") {
        out.push(Found::code("awk `system`: runs a command"));
    }
    let named_after_at = |(at, _): (usize, &str)| {
        (program[at + 1..].chars().next()).is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    };
    if program.match_indices('@').any(named_after_at) {
        out.push(Found::code(
            "awk `@`: loads an extension or code, or calls a function by its name",
        ));
    }
    // A single `|` (not `||`) is a pipe to or from a command.
    let bytes = program.as_bytes();
    for (k, b) in bytes.iter().enumerate() {
        if *b == b'|' && bytes.get(k + 1) != Some(&b'|') && (k == 0 || bytes[k - 1] != b'|') {
            out.push(Found::code("awk `|`: pipes to or from a command"));
            break;
        }
    }
    // `print > "file"` / `printf >> "file"`: a `>` is a redirect only in a
    // print statement — anywhere else (`$1 > 5 { … }`, `(getline x) > 0`)
    // it compares. The statement is the text since the last `{`, `;` or
    // newline; `>=` always compares.
    let mut seen = 0;
    while let Some(at) = program[seen..].find('>') {
        let at = seen + at;
        let statement_start = program[..at].rfind(['{', ';', '\n']).map_or(0, |k| k + 1);
        let in_print = program[statement_start..at].contains("print");
        let after = program[at + 1..].trim_start_matches('>').trim_start();
        if in_print && !after.starts_with('=') {
            if let Some(quoted) = after.strip_prefix('"') {
                let target: String = quoted.chars().take_while(|c| *c != '"').collect();
                out.push(Found::file(Role::WriteFile, target));
            } else if !after.is_empty() {
                out.push(Found::file(Role::WriteFile, DYNAMIC));
            }
        }
        seen = at + 1;
    }
    // `getline < "file"` and `getline VAR < "file"`.
    let mut rest = program;
    while let Some(at) = rest.find("getline") {
        let after = rest[at + "getline".len()..].trim_start();
        // An optional variable to read into: an identifier, or a `$field`.
        let after = after
            .trim_start_matches(|c: char| c == '$' || c.is_ascii_alphanumeric() || c == '_')
            .trim_start();
        if let Some(src) = after.strip_prefix('<') {
            let src = src.trim_start();
            if let Some(quoted) = src.strip_prefix('"') {
                out.push(Found::file(
                    Role::ReadFile,
                    quoted.chars().take_while(|c| *c != '"').collect::<String>(),
                ));
            } else {
                out.push(Found::file(Role::ReadFile, DYNAMIC));
            }
        }
        rest = &rest[at + "getline".len()..];
    }
    out
}

/// Is `word` in `program` as a whole identifier — not a piece of a longer
/// one (`subsystem`, `system_x`)?
fn has_word(program: &str, word: &str) -> bool {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    program.match_indices(word).any(|(at, _)| {
        let before = program[..at].chars().next_back();
        let after = program[at + word.len()..].chars().next();
        !before.is_some_and(ident) && !after.is_some_and(ident)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Printing fields finds nothing.**
    #[test]
    fn a_print_program_is_a_read() {
        for p in [
            "{print $1}",
            "{ s += $2 } END { print s }",
            "$1 > 5 { print }",
            "NR==1",
        ] {
            assert!(scan(p).is_empty(), "{p}: {:?}", scan(p));
        }
    }

    /// **`system`, `@load` and a pipe run code.**
    #[test]
    fn code_is_found() {
        assert_eq!(scan("{ system(\"ls\") }")[0].role, Role::Code);
        assert_eq!(scan("@load \"x\"")[0].role, Role::Code);
        assert_eq!(scan("{ print | \"sort\" }")[0].role, Role::Code);
        assert_eq!(scan("{ \"date\" | getline d }")[0].role, Role::Code);
        assert!(scan("$1 || $2").is_empty(), "`||` is an or, not a pipe");
    }

    /// **`system` is code however its call is spelled, and so is an `@`
    /// before a name** — the spellings awk takes (a space, a tab, a line
    /// continuation before the parenthesis; an indirect call through a
    /// string, `@include`); a longer name holding `system` is not it.
    #[test]
    fn code_is_found_however_the_call_is_spelled() {
        for p in [
            "{ system(\"ls\") }",
            "{ system (\"ls\") }",
            "{ system\t(\"ls\") }",
            "{ x = system   (\"ls\") }",
            "{ system \\\n (\"ls\") }",
            "BEGIN { f = \"sys\" \"tem\"; @f(\"ls\") }",
            "@include \"lib.awk\"",
            "@load \"x\"",
            // In a string too: the reader does not parse awk, and asking
            // is the safe side of a word it cannot place.
            "{ print \"a@b system\" }",
        ] {
            assert!(
                scan(p).iter().any(|f| f.role == Role::Code),
                "{p:?}: {:?}",
                scan(p)
            );
        }
        for p in [
            "{ print subsystem }",
            "{ system_x = 1; print system_x }",
            "{ print \"user at host\" }",
        ] {
            assert!(scan(p).is_empty(), "{p:?}: {:?}", scan(p));
        }
    }

    /// **A redirect writes the named file; a dynamic target is placed
    /// nowhere.**
    #[test]
    fn redirects_name_their_files() {
        let f = scan("{ print > \"/tmp/out\" }");
        assert_eq!(
            (f[0].role, f[0].what.as_str()),
            (Role::WriteFile, "/tmp/out")
        );
        let f = scan("{ print >> \"log\" }");
        assert_eq!((f[0].role, f[0].what.as_str()), (Role::WriteFile, "log"));
        let f = scan("{ print > $2 }");
        assert_eq!((f[0].role, f[0].what.as_str()), (Role::WriteFile, DYNAMIC));
        assert!(scan("$1 >= 3").is_empty(), "`>=` compares");
        let f = scan("{ while ((getline line < \"/etc/hosts\") > 0) n++ }");
        assert!(
            f.iter()
                .any(|x| x.role == Role::ReadFile && x.what == "/etc/hosts"),
            "{f:?}"
        );
    }
}
