//! **Text that reads as something it is not** — the characters, beyond the
//! controls (`char::is_control`), that make a line display other than it is
//! written ([`misleads`]); those that draw as nothing, which only a token
//! must not hold ([`draws_as_nothing`]); and the one way a line shows the
//! first instead ([`escaped`]). Their consumers: a notification's door
//! (`ring::door`, the TUI's: drops), the `:ls` glyph (`read_glyph`:
//! refuses), a session's label ([`Label`](crate::graph::Label): a
//! description drops, a name refuses both), the config notes (`legible`:
//! shows both) and every notice on the status row (`App::post`: shows) —
//! each says what it does with such a character.
//!
//! **Every language reads as written.** The rule names what reorders, breaks
//! or hides text, never a script: Korean, Chinese and every other letter,
//! accents and every combining mark, the joiners Arabic-script and Indic
//! text are shaped with (U+200C/D, and emoji joined with them), the
//! zero-width space Thai and Burmese break words at, the bidi marks
//! right-to-left text places its punctuation with (U+200E/F, U+061C),
//! variation selectors — none of them is in it (review stop 9, E2; Steven,
//! 10-24: "we do need to handle accents and other language text
//! characters"). The three subdivision flags are the one exception, and
//! [`misleads`] says why.

/// **Does `c` make text read as something it is not?** — a character that
/// is not a control but still reorders what follows it, starts a line where
/// a newline filter sees none, or carries text no one sees:
///
/// - the bidi embeddings, overrides and isolates (U+202A–E, U+2066–9): what
///   follows them is shown reordered — `txt.exe` after a U+202E reads
///   `exe.txt`
///   (CVE-2021-42574, "Trojan Source"; rustc's `text_direction_codepoint`
///   lints name these nine);
/// - the line and paragraph separators (U+2028, U+2029): a second line in a
///   notification whose words have no newline;
/// - the deprecated format controls (U+206A–F: symmetric swapping, shaping),
///   which reorder or reshape by state no one can see;
/// - interlinear annotation (U+FFF9–B), whose annotation a renderer may hide;
/// - tags (U+E0000–7F): invisible ASCII, the shape text is smuggled in. The
///   one emoji written with them is caught too, knowingly: the England,
///   Scotland and Wales flags are U+1F3F4 and a tag run, which reaches a
///   notification as a bare 🏴 and a notice as its escapes — a sequence
///   check for three flags against every run of hidden text it would have
///   to tell from them (review stop 9's verification, G 6).
pub fn misleads(c: char) -> bool {
    matches!(
        c,
        '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{206A}'..='\u{206F}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

/// **Does `c` draw as nothing?** — the zero-widths and joiners (U+200B–D,
/// U+2060–4), the bidi marks (U+200E–F, U+061C), the BOM, a soft hyphen,
/// the Mongolian vowel separator. Language text holds them — they join,
/// break and place words — so text keeps them; but a token you type back
/// or must fix cannot hide one: a session's name (`Label`), refused, and a
/// config note quoting what a file wrote (`legible`), shown as its escape,
/// so `queued` and `queued` plus U+200B do not read alike.
pub fn draws_as_nothing(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{2060}'..='\u{2064}'
            | '\u{FEFF}'
    )
}

/// **`text`, each character that [`misleads`] shown as its escape** —
/// `\u{202e}` where it would have reordered the line silently — and the
/// rest as written, every language's letters and marks among it.
pub fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if misleads(c) {
            out.extend(c.escape_unicode());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **What misleads is caught, and every language is not** — each class
    /// named on [`misleads`], at both ends of its range; and text in
    /// Korean, Chinese, with accents (precomposed and combining), Persian
    /// with its non-joiner, Hindi with its joiner, Thai with its zero-width
    /// space, Hebrew with a bidi mark, an emoji sequence and a CJK variation
    /// selector, all passing as written.
    #[test]
    fn what_misleads_is_caught_and_every_language_is_not() {
        for c in [
            '\u{202A}',
            '\u{202E}',
            '\u{2066}',
            '\u{2069}',
            '\u{2028}',
            '\u{2029}',
            '\u{206A}',
            '\u{206F}',
            '\u{FFF9}',
            '\u{FFFB}',
            '\u{E0000}',
            '\u{E0041}',
            '\u{E007F}',
        ] {
            assert!(misleads(c), "{:?}", c.escape_unicode().to_string());
        }
        let languages = [
            "인증 수정",
            "修复认证",
            "café, cafe\u{301}",
            "می\u{200C}خواهم",
            "क्\u{200D}ष",
            "สวัสดี\u{200B}ครับ",
            "שלום\u{200F}!",
            "\u{1F468}\u{200D}\u{1F4BB}",
            "\u{845B}\u{E0100}",
            "\u{0661}\u{061C}\u{0662}",
        ];
        for text in languages {
            assert!(!text.chars().any(misleads), "{text}");
            assert_eq!(escaped(text), text, "as written");
        }
    }

    /// **What draws as nothing is caught, at both ends of each range, and its
    /// neighbours are not** — `misleads`' table, mirrored: it had no test of
    /// its own, and dropping the BOM let one inside a session's name read
    /// back clean (review stop 12, T9).
    #[test]
    fn what_draws_as_nothing_is_caught_and_its_neighbours_are_not() {
        for c in [
            '\u{00AD}', '\u{061C}', '\u{180E}', '\u{200B}', '\u{200F}', '\u{2060}', '\u{2064}',
            '\u{FEFF}',
        ] {
            assert!(draws_as_nothing(c), "{:?}", c.escape_unicode().to_string());
        }
        for c in [
            '\u{00AC}', '\u{061B}', '\u{200A}', '\u{2010}', '\u{205F}', '\u{2065}', 'a', ' ',
        ] {
            assert!(!draws_as_nothing(c), "{:?}", c.escape_unicode().to_string());
        }
    }

    /// **Shown, not hidden**: a reordering override in a line reads as its
    /// escape, the line around it as written.
    #[test]
    fn a_reordering_character_is_shown_as_its_escape() {
        assert_eq!(
            escaped("news in b1 \u{202E}txt.exe"),
            "news in b1 \\u{202e}txt.exe"
        );
        assert_eq!(escaped("a\u{2028}b"), "a\\u{2028}b");
    }
}
