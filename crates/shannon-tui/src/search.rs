//! **Search patterns** — what `/` and `?` read (Steven, 10-07: "We should
//! prioritize searching").
//!
//! A pattern is a Rust regex, the dialect the gate's `re:` rules already
//! speak, not vim's: `foo.*bar` and `a|b` mean what they say there. Two of
//! vim's atoms are read as vim reads them, being the ones a search is most
//! often typed with: `\<` and `\>` (a word's edges) and `\c` / `\C` (ignore
//! or match case, anywhere in the pattern). Otherwise case is as [`Case`]
//! says — vim's `'ignorecase'` and `'smartcase'`, both off by default, so
//! matching is case-SENSITIVE unless you set them.

/// The most a compiled pattern may take — the regex crate's own bound on
/// what one typed line can build.
const SIZE_LIMIT: usize = 1 << 20;

/// **How a pattern that says nothing of case is matched** — `[tui]
/// ignorecase` and `smartcase`, vim's options of those names (Steven, 10-30:
/// "We'll add the options but keep defaults like they are in vim").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Case {
    /// Case ignored — `'ignorecase'`.
    pub(crate) ignore: bool,
    /// …but matched for a pattern with a capital in it — `'smartcase'`,
    /// only with `ignore`. False for `*` and `#`: their word was not typed.
    pub(crate) smart: bool,
}

impl Case {
    /// The case `[tui]` sets, for a pattern `typed` at `/` or `?` or not.
    pub(crate) fn of(tui: &shannon_core::config::TuiConfig, typed: bool) -> Case {
        Case {
            ignore: tui.ignorecase,
            smart: tui.smartcase && typed,
        }
    }
}

/// **The last search, as `n` repeats it** — vim's search register, with
/// what vim keeps beside it (`no_scs`): whether the pattern was typed, so
/// `n` after `*` matches case as `*` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LastSearch {
    pub(crate) pattern: String,
    /// The way `/` (forward) or `?` went; `N` reverses it.
    pub(crate) forward: bool,
    /// Typed at `/` or `?`, not taken from the cursor by `*` or `#`.
    pub(crate) typed: bool,
}

/// **What a search that found nothing says** — vim's E486, and, where the
/// pattern holds a `^` or `$` no line can match where it stands, what it
/// matches there and the escape that finds the character (S21-Q14:
/// `/CTRL-^` found nothing, review stop 15 F M5). The pattern keeps its
/// reading: `grep -E`, `awk` and `rg` read a mid-pattern `^` as an anchor
/// too, vim and plain `grep` as the character (measured, S21-Q9; Steven:
/// leave it).
pub(crate) fn not_found(pattern: &str) -> String {
    let said = format!("pattern not found: {pattern}");
    let Some(anchor) = misplaced_anchor(pattern) else {
        return said;
    };
    let (edge, character) = match anchor {
        '^' => ("start", "a caret"),
        _ => ("end", "a dollar sign"),
    };
    format!(
        "{said} — `{anchor}` mid-pattern matches a line's {edge}, not {character}; \
         `\\{anchor}` finds {character}"
    )
}

/// **A `^` or `$` where no line has an edge** — a `^` past the pattern's
/// start, a group's or an alternative's, a `$` before its end, a group's or
/// an alternative's; never one escaped, nor inside a `[…]`, where `^`
/// negates.
fn misplaced_anchor(pattern: &str) -> Option<char> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut class = false;
    let mut at = 0;
    while let Some(&c) = chars.get(at) {
        let before = at.checked_sub(1).map(|b| chars[b]);
        let after = chars.get(at + 1).copied();
        match c {
            '\\' => at += 1,
            '[' => class = true,
            ']' => class = false,
            '^' if !class && !matches!(before, None | Some('(' | '|')) => return Some('^'),
            '$' if !class && !matches!(after, None | Some(')' | '|')) => return Some('$'),
            _ => {}
        }
        at += 1;
    }
    None
}

/// **`pattern`, compiled for a search** — or why it is not one, in a line.
/// A `\c` decides case for the whole pattern, else a `\C`, else `case`
/// (vim's order).
pub(crate) fn compile(pattern: &str, case: Case) -> Result<regex::Regex, String> {
    let read = Read::of(pattern);
    let ignore = match (read.ignore, read.exact) {
        (true, _) => true,
        (false, true) => false,
        (false, false) => case.ignore && !(case.smart && read.capital),
    };
    regex::RegexBuilder::new(&read.text)
        .case_insensitive(ignore)
        .size_limit(SIZE_LIMIT)
        .build()
        .map_err(|e| {
            // The crate's message is several lines (the pattern, a caret);
            // the last says what is wrong.
            let said = e.to_string();
            let why = said.lines().last().unwrap_or("").trim();
            let why = why.strip_prefix("error: ").unwrap_or(why);
            format!("`{pattern}` is not a pattern — {why}")
        })
}

/// **A typed pattern, read once** — the crate's regex it stands for, and
/// what it says of case.
struct Read {
    /// The pattern with vim's atoms in the crate's spelling, `\c` and `\C`
    /// gone.
    text: String,
    /// It holds a `\c`.
    ignore: bool,
    /// It holds a `\C`.
    exact: bool,
    /// A capital outside an escape — what `'smartcase'` asks of it. An
    /// escape is not one (`\S`, `\p{Lu}`), as vim's `pat_has_uppercase`
    /// skips `\X`.
    capital: bool,
}

impl Read {
    /// One walk, escape by escape, so a `\\` is one escape — a `\\c` is a
    /// backslash and a `c`, never the switch (a text replace took it as
    /// one).
    fn of(pattern: &str) -> Read {
        let mut read = Read {
            text: String::with_capacity(pattern.len()),
            ignore: false,
            exact: false,
            capital: false,
        };
        let mut chars = pattern.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\\' {
                read.capital |= c.is_uppercase();
                read.text.push(c);
                continue;
            }
            match chars.next() {
                Some('c') => read.ignore = true,
                Some('C') => read.exact = true,
                Some('<' | '>') => read.text.push_str("\\b"),
                Some(escaped) => {
                    read.text.push('\\');
                    read.text.push(escaped);
                    // `\p{Lu}`, `\x{1F}`: the braces are the escape's.
                    if chars.peek() == Some(&'{') {
                        for b in chars.by_ref() {
                            read.text.push(b);
                            if b == '}' {
                                break;
                            }
                        }
                    }
                }
                // A trailing `\`, left for the crate to say why.
                None => read.text.push('\\'),
            }
        }
        read
    }
}

/// **What `*` and `#` search for**: `word` whole — between word edges when it
/// is a keyword (`[[:alnum:]_]`), as written otherwise — vim's rule.
pub(crate) fn word_pattern(word: &str, keyword: bool) -> String {
    let escaped = regex::escape(word);
    match keyword {
        true => format!("\\b{escaped}\\b"),
        false => escaped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIM: Case = Case {
        ignore: false,
        smart: false,
    };
    const IGNORE: Case = Case {
        ignore: true,
        smart: false,
    };
    const SMART: Case = Case {
        ignore: true,
        smart: true,
    };

    #[test]
    fn a_pattern_reads_vims_case_switches_and_word_edges() {
        assert!(compile("Help", VIM).unwrap().is_match("Help"));
        assert!(
            !compile("Help", VIM).unwrap().is_match("help"),
            "vim's default"
        );
        assert!(compile("\\chelp", VIM).unwrap().is_match("HELP"));
        // `\C` is no literal, and forces case over 'ignorecase' — under
        // vim's default it held whether `\C` did anything or not (review
        // stop 14, C 11).
        let ignoring = Case {
            ignore: true,
            smart: false,
        };
        let sensitive = compile("help\\C", ignoring).unwrap();
        assert!(
            sensitive.is_match("help") && !sensitive.is_match("HELP"),
            "\\C"
        );
        let edges = compile("\\<rule\\>", VIM).unwrap();
        assert!(edges.is_match("a rule here") && !edges.is_match("rules"));
    }

    /// **vim's `'ignorecase'` and `'smartcase'`** — Steven's `/mara` finds
    /// `Mara`, his `/Mara` only `Mara`; `\c` and `\C` decide over both, `\c`
    /// first.
    #[test]
    fn case_is_vims_ignorecase_and_smartcase() {
        let finds = |pattern, case, text| compile(pattern, case).unwrap().is_match(text);
        assert!(finds("mara", IGNORE, "Mara"), "ignorecase");
        assert!(
            finds("Mara", IGNORE, "mara"),
            "ignorecase alone ignores a capital"
        );
        assert!(
            finds("mara", SMART, "Mara"),
            "smartcase, no capital: ignored"
        );
        assert!(
            !finds("Mara", SMART, "mara"),
            "smartcase, a capital: matched"
        );
        assert!(finds("Mara", SMART, "Mara"));
        let smart_alone = Case {
            ignore: false,
            smart: true,
        };
        assert!(
            !finds("mara", smart_alone, "Mara"),
            "smartcase needs ignorecase"
        );
        assert!(!finds("mara\\C", SMART, "Mara"), "\\C over both");
        assert!(finds("Mara\\c", VIM, "mara"), "\\c over both");
        assert!(finds("\\cMara\\C", VIM, "mara"), "\\c before \\C");
    }

    /// **An escape is not a capital** — `\S` is no `S` to `'smartcase'`, as
    /// vim's `pat_has_uppercase` skips `\X` — and **`\\` is one escape**: a
    /// `\\c` is a backslash then `c`, not the switch.
    #[test]
    fn a_pattern_is_read_escape_by_escape() {
        let finds = |pattern, case, text| compile(pattern, case).unwrap().is_match(text);
        assert!(
            finds("\\Sara", SMART, "MARA"),
            "no capital but the escape's"
        );
        assert!(finds("\\p{Lu}ara", SMART, "MARA"), "nor in its braces");
        assert!(!finds("\\SAra", SMART, "MARA"), "a capital after it counts");
        assert!(finds("a\\\\c", VIM, "a\\c"), "a backslash, then c");
        assert!(!finds("a\\\\c", VIM, "a"), "not the switch");
    }

    #[test]
    fn a_bad_pattern_says_why_in_a_line() {
        let why = compile("a(b", VIM).unwrap_err();
        assert!(why.starts_with("`a(b` is not a pattern — "), "{why}");
        assert!(!why.contains('\n'), "{why}");
        let trailing = compile("a\\", VIM).unwrap_err();
        assert!(
            trailing.starts_with("`a\\` is not a pattern — "),
            "{trailing}"
        );
    }

    /// **A `^` or `$` no line can match where it stands is named** in what a
    /// search that found nothing says (S21-Q14) — never one at an edge, a
    /// group's or an alternative's, one escaped, nor a class's negation.
    #[test]
    fn a_search_that_found_nothing_names_a_misplaced_anchor() {
        for at_an_edge in [
            "^foo", "foo$", "^foo$", "(^a|^b)", "(a$|b$)", "a|^b", "[^a]b", "a\\^b", "a\\$b",
        ] {
            assert_eq!(misplaced_anchor(at_an_edge), None, "{at_an_edge}");
        }
        assert_eq!(misplaced_anchor("CTRL-^"), Some('^'));
        assert_eq!(misplaced_anchor("a$b"), Some('$'));
        assert_eq!(not_found("zz"), "pattern not found: zz");
        let said = not_found("CTRL-^");
        assert!(said.starts_with("pattern not found: CTRL-^ — "), "{said}");
        assert!(
            said.contains("`^` mid-pattern matches a line's start") && said.contains("`\\^`"),
            "{said}"
        );
        assert!(not_found("a$b").contains("`\\$` finds a dollar sign"));
    }

    #[test]
    fn a_word_is_searched_whole_and_as_written() {
        let kw = compile(&word_pattern("w", true), VIM).unwrap();
        assert!(kw.is_match("a w b") && !kw.is_match("aw"));
        let other = compile(&word_pattern("a.b", false), VIM).unwrap();
        assert!(other.is_match("x a.b") && !other.is_match("axb"));
    }
}
